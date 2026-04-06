#!/usr/bin/env python3
"""
recognize.py — Card recognition engine for the Elmo OX-1 / FI-8040.

Modes:
    python recognize.py test <image.jpg>         # identify a single image
    python recognize.py watch <folder>           # watch folder for new images
    python recognize.py watch <folder> --ingest  # watch + auto-POST to /api/ingest

How it works:
  1. Detect card boundary — find the largest 4-sided contour in the image.
  2. Perspective-correct to a standard portrait (630×880).
  3. Crop art region (same percentages as hash_cards.py).
  4. Compute dHash (8×8 = 64-bit).
  5. Load all hashes from card_hashes table into memory.
  6. Find closest match by Hamming distance.
  7. If distance ≤ threshold (default 12), accept the match.
  8. In watch mode, pair consecutive images as front/back within a 10s window.

In watch+ingest mode, POST to /api/ingest with:
  - scryfall_id, condition (prompted or default), foil (default 'normal')
  - front image (first capture), back image (second capture, optional)

Requirements: pip install -r requirements.txt
"""

import argparse
import os
import re
import sqlite3
import sys
import time
import io
import json
from pathlib import Path
from threading import Lock

import cv2
import numpy as np
import requests
from PIL import Image
import imagehash
from dotenv import load_dotenv
from watchdog.observers import Observer
from watchdog.events import FileSystemEventHandler

# ---------------------------------------------------------------------------
# Constants (must match hash_cards.py)
# ---------------------------------------------------------------------------

ART_Y_TOP    = 0.04
ART_Y_BOTTOM = 0.96
ART_X_LEFT   = 0.04
ART_X_RIGHT  = 0.96
HASH_SIZE    = 8

CARD_W = 630    # standard portrait width for perspective correction
CARD_H = 880    # standard portrait height

MATCH_THRESHOLD = 12   # Hamming distance ≤ this → confident match (out of 64)
PAIR_WINDOW     = 10.0 # seconds between captures to count as a front/back pair

# ---------------------------------------------------------------------------
# Image processing
# ---------------------------------------------------------------------------

def find_card_contour(img_bgr: np.ndarray) -> np.ndarray | None:
    """
    Locate the card in the image.
    Returns a (4,2) float32 array of corner points (TL, TR, BR, BL), or None.
    """
    gray = cv2.cvtColor(img_bgr, cv2.COLOR_BGR2GRAY)
    blurred = cv2.GaussianBlur(gray, (5, 5), 0)
    edged = cv2.Canny(blurred, 30, 100)

    # Dilate edges slightly to close small gaps
    kernel = np.ones((3, 3), np.uint8)
    edged = cv2.dilate(edged, kernel, iterations=1)

    contours, _ = cv2.findContours(edged, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
    if not contours:
        return None

    # Sort by area descending
    contours = sorted(contours, key=cv2.contourArea, reverse=True)

    ih, iw = img_bgr.shape[:2]
    min_area = (iw * ih) * 0.05   # card must be at least 5% of frame

    for cnt in contours[:5]:
        area = cv2.contourArea(cnt)
        if area < min_area:
            break

        peri = cv2.arcLength(cnt, True)
        approx = cv2.approxPolyDP(cnt, 0.02 * peri, True)

        if len(approx) == 4:
            return order_points(approx.reshape(4, 2).astype(np.float32))

    return None


def order_points(pts: np.ndarray) -> np.ndarray:
    """Order 4 points as: top-left, top-right, bottom-right, bottom-left."""
    rect = np.zeros((4, 2), dtype=np.float32)
    s = pts.sum(axis=1)
    rect[0] = pts[np.argmin(s)]   # TL
    rect[2] = pts[np.argmax(s)]   # BR
    diff = np.diff(pts, axis=1)
    rect[1] = pts[np.argmin(diff)]  # TR
    rect[3] = pts[np.argmax(diff)]  # BL
    return rect


def perspective_warp(img_bgr: np.ndarray, corners: np.ndarray) -> np.ndarray:
    """Apply perspective transform to produce a CARD_W × CARD_H image."""
    dst = np.array([
        [0, 0],
        [CARD_W - 1, 0],
        [CARD_W - 1, CARD_H - 1],
        [0, CARD_H - 1],
    ], dtype=np.float32)
    M = cv2.getPerspectiveTransform(corners, dst)
    return cv2.warpPerspective(img_bgr, M, (CARD_W, CARD_H))


def crop_art_region(img_bgr: np.ndarray) -> np.ndarray:
    """Crop the art region from a perspective-corrected card image."""
    h, w = img_bgr.shape[:2]
    x1 = int(w * ART_X_LEFT)
    x2 = int(w * ART_X_RIGHT)
    y1 = int(h * ART_Y_TOP)
    y2 = int(h * ART_Y_BOTTOM)
    return img_bgr[y1:y2, x1:x2]


def compute_dhash_from_array(img_bgr: np.ndarray) -> str:
    """Compute dHash from a BGR numpy array."""
    rgb = cv2.cvtColor(img_bgr, cv2.COLOR_BGR2RGB)
    pil = Image.fromarray(rgb)
    return str(imagehash.dhash(pil, hash_size=HASH_SIZE))


def hamming_distance(h1: str, h2: str) -> int:
    """Compute Hamming distance between two hex-encoded hashes."""
    try:
        i1 = int(h1, 16)
        i2 = int(h2, 16)
        return bin(i1 ^ i2).count('1')
    except (ValueError, TypeError):
        return 64  # max distance on error


# ---------------------------------------------------------------------------
# Hash database
# ---------------------------------------------------------------------------

class HashIndex:
    """In-memory index of all card hashes loaded from SQLite."""

    def __init__(self, db_path: str):
        self.db_path = db_path
        self.hashes: list[tuple[str, str]] = []   # [(scryfall_id, dhash), ...]
        self.card_info: dict[str, dict] = {}       # scryfall_id → {name, set_code, ...}
        self._lock = Lock()
        self.load()

    def load(self):
        """Load all hashes from card_hashes JOIN scryfall_cards."""
        conn = sqlite3.connect(self.db_path)
        conn.row_factory = sqlite3.Row
        rows = conn.execute("""
            SELECT ch.scryfall_id, ch.dhash, sc.name, sc.set_code, sc.set_name,
                   sc.collector_number, sc.rarity, sc.image_uri
            FROM card_hashes ch
            JOIN scryfall_cards sc ON sc.scryfall_id = ch.scryfall_id
        """).fetchall()
        conn.close()

        with self._lock:
            self.hashes = [(r["scryfall_id"], r["dhash"]) for r in rows]
            self.card_info = {
                r["scryfall_id"]: {
                    "name": r["name"],
                    "set_code": r["set_code"],
                    "set_name": r["set_name"],
                    "collector_number": r["collector_number"],
                    "rarity": r["rarity"],
                    "image_uri": r["image_uri"],
                }
                for r in rows
            }

        print(f"[index] Loaded {len(self.hashes)} card hashes")

    def find_best(self, query_hash: str, top_n: int = 3) -> list[tuple[int, str, dict]]:
        """
        Find the top_n closest matches.
        Returns list of (distance, scryfall_id, card_info) sorted by distance.
        """
        with self._lock:
            scored = [
                (hamming_distance(query_hash, dhash), sid)
                for sid, dhash in self.hashes
            ]
        scored.sort(key=lambda x: x[0])
        results = []
        for dist, sid in scored[:top_n]:
            results.append((dist, sid, self.card_info.get(sid, {})))
        return results


# ---------------------------------------------------------------------------
# Recognition
# ---------------------------------------------------------------------------

def recognize_image(img_path: str, index: HashIndex, verbose: bool = True) -> dict | None:
    """
    Identify a card in an image file.
    Returns match dict {scryfall_id, name, set_code, distance, confident, ...}
    or None if the card can't be detected/matched.
    """
    img_bgr = cv2.imread(img_path)
    if img_bgr is None:
        if verbose:
            print(f"  [error] Cannot read image: {img_path}")
        return None

    if verbose:
        h, w = img_bgr.shape[:2]
        print(f"  Image: {w}×{h}")

    # Try to find card contour
    corners = find_card_contour(img_bgr)

    if corners is not None:
        if verbose:
            print(f"  Card detected via contour detection")
        warped = perspective_warp(img_bgr, corners)
    else:
        if verbose:
            print(f"  No clean contour — using full image with central crop")
        # Fall back: assume card fills most of frame, apply a center crop
        h, w = img_bgr.shape[:2]
        margin_x = int(w * 0.05)
        margin_y = int(h * 0.05)
        warped = img_bgr[margin_y:h - margin_y, margin_x:w - margin_x]
        # Resize to standard card dimensions
        warped = cv2.resize(warped, (CARD_W, CARD_H))

    art = crop_art_region(warped)
    query_hash = compute_dhash_from_array(art)

    if verbose:
        print(f"  Query hash: {query_hash}")

    if not index.hashes:
        if verbose:
            print(f"  [warn] No hashes in index — run hash_cards.py first")
        return None

    matches = index.find_best(query_hash, top_n=3)

    if verbose:
        print(f"  Top matches:")
        for dist, sid, info in matches:
            flag = "✓" if dist <= MATCH_THRESHOLD else "✗"
            print(f"    {flag} dist={dist:2d}  {info.get('name','?'):30s}  [{info.get('set_code','?')}]  {sid}")

    best_dist, best_sid, best_info = matches[0]
    confident = best_dist <= MATCH_THRESHOLD

    return {
        "scryfall_id": best_sid,
        "name": best_info.get("name", ""),
        "set_code": best_info.get("set_code", ""),
        "set_name": best_info.get("set_name", ""),
        "collector_number": best_info.get("collector_number", ""),
        "rarity": best_info.get("rarity", ""),
        "image_uri": best_info.get("image_uri", ""),
        "distance": best_dist,
        "confident": confident,
        "query_hash": query_hash,
        "top3": [
            {"dist": d, "scryfall_id": s, "name": i.get("name", ""), "set_code": i.get("set_code", "")}
            for d, s, i in matches
        ],
    }


# ---------------------------------------------------------------------------
# Watch mode
# ---------------------------------------------------------------------------

class CardCapturePair:
    """Tracks front/back pairs within the time window."""
    def __init__(self):
        self.front_path: str | None = None
        self.front_result: dict | None = None
        self.front_time: float = 0.0
        self.lock = Lock()

    def submit(self, path: str, result: dict) -> tuple | None:
        """
        Submit a recognized image. If it pairs with a recent front, return (front, back).
        Otherwise, store as the new front and return None.
        """
        now = time.monotonic()
        with self.lock:
            if self.front_path and (now - self.front_time) < PAIR_WINDOW:
                # This is the back of the card
                pair = (self.front_path, self.front_result, path, result)
                self.front_path = None
                self.front_result = None
                return pair
            else:
                # This is a new front
                self.front_path = path
                self.front_result = result
                self.front_time = now
                return None


def prompt_condition() -> str:
    """Prompt user for condition code."""
    print("\n  Condition: [N]M / [L]P / [M]P / [H]P / [D]amaged  (default: NM)")
    key = input("  > ").strip().upper()
    mapping = {"N": "near_mint", "L": "lightly_played", "M": "moderately_played",
               "H": "heavily_played", "D": "damaged", "": "near_mint",
               "NM": "near_mint", "LP": "lightly_played", "MP": "moderately_played",
               "HP": "heavily_played", "DMG": "damaged"}
    return mapping.get(key, "near_mint")


def prompt_foil() -> str:
    """Prompt user for foil type."""
    print("  Foil: [n]ormal / [f]oil / [e]tched  (default: normal)")
    key = input("  > ").strip().lower()
    if key in ("f", "foil"):
        return "foil"
    elif key in ("e", "etched"):
        return "etched"
    return "normal"


def post_ingest(server_url: str, scryfall_id: str, condition: str, foil: str,
                front_path: str, back_path: str | None) -> dict | None:
    """POST to /api/ingest with card data and scan images."""
    url = f"{server_url.rstrip('/')}/api/ingest"
    try:
        files = {
            "scryfall_id": (None, scryfall_id),
            "condition": (None, condition),
            "foil": (None, foil),
        }
        if front_path and os.path.exists(front_path):
            files["front"] = (os.path.basename(front_path), open(front_path, "rb"), "image/jpeg")
        if back_path and os.path.exists(back_path):
            files["back"] = (os.path.basename(back_path), open(back_path, "rb"), "image/jpeg")

        resp = requests.post(url, files=files, timeout=15)
        if resp.status_code == 200:
            return resp.json()
        else:
            print(f"  [ingest error] HTTP {resp.status_code}: {resp.text[:200]}")
            return None
    except Exception as e:
        print(f"  [ingest error] {e}")
        return None


class ScanHandler(FileSystemEventHandler):
    """Watchdog handler: process new image files dropped in the watch folder."""

    EXTENSIONS = {".jpg", ".jpeg", ".png", ".tif", ".tiff", ".bmp"}

    def __init__(self, index: HashIndex, pair_tracker: CardCapturePair,
                 server_url: str | None, auto_ingest: bool, condition: str | None):
        self.index = index
        self.pairs = pair_tracker
        self.server_url = server_url
        self.auto_ingest = auto_ingest
        self.default_condition = condition or "near_mint"

    def on_created(self, event):
        if event.is_directory:
            return
        path = event.src_path
        ext = Path(path).suffix.lower()
        if ext not in self.EXTENSIONS:
            return

        # Small delay to ensure file is fully written
        time.sleep(0.3)
        self._handle_image(path)

    def _handle_image(self, path: str):
        print(f"\n{'='*60}")
        print(f"New image: {path}")
        result = recognize_image(path, self.index, verbose=True)

        if not result:
            print("  Could not process image.")
            return

        if not result["confident"]:
            print(f"  ⚠  Low confidence (dist={result['distance']}). Best guess: {result['name']} [{result['set_code']}]")
            print("     Skipping — manual entry required.")
            return

        print(f"  ✓ MATCH: {result['name']} [{result['set_code']}#{result['collector_number']}]  dist={result['distance']}")

        pair = self.pairs.submit(path, result)

        if pair is None:
            # Stored as front — waiting for back
            print(f"  Stored as FRONT. Scan the back within {PAIR_WINDOW:.0f}s, or scan next card.")
            return

        front_path, front_result, back_path, back_result = pair
        print(f"\n  Front: {front_result['name']} (dist={front_result['distance']})")
        print(f"  Back: {back_result['name']} (dist={back_result['distance']})")

        # Use the front identification (back of card has no unique art in standard layout)
        final_result = front_result

        if self.auto_ingest and self.server_url:
            condition = self.default_condition
            foil = "normal"
            ingest_result = post_ingest(
                self.server_url, final_result["scryfall_id"],
                condition, foil, front_path, back_path
            )
            if ingest_result and ingest_result.get("ok"):
                card_id = ingest_result.get("card_id")
                print(f"  ✓ Ingested: {final_result['name']} → ID {card_id}")
            else:
                print(f"  ✗ Ingest failed: {ingest_result}")
        else:
            print(f"  Ready to ingest: {final_result['scryfall_id']} ({final_result['name']})")
            print(f"  Run with --ingest to auto-POST to card-vault")


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def load_db_path(override: str | None) -> str:
    if override:
        return override
    env_file = Path(__file__).parent.parent / ".env"
    if env_file.exists():
        load_dotenv(env_file)
    db_url = os.environ.get("DATABASE_URL", "sqlite:./card-vault.db")
    db_path = re.sub(r'^sqlite:', '', db_url).lstrip('/')
    if not os.path.isabs(db_path):
        db_path = str(Path(__file__).parent.parent / db_path)
    return db_path


def main():
    parser = argparse.ArgumentParser(description="MTG card recognition for Elmo OX-1 / FI-8040")
    sub = parser.add_subparsers(dest="command", required=True)

    # test subcommand
    test_p = sub.add_parser("test", help="Identify a single image file")
    test_p.add_argument("image", help="Path to image file")
    test_p.add_argument("--db", default=None)
    test_p.add_argument("--threshold", type=int, default=MATCH_THRESHOLD,
                        help=f"Hamming distance threshold (default: {MATCH_THRESHOLD})")
    test_p.add_argument("--save-art", action="store_true",
                        help="Save cropped art region to art_crop_debug.jpg")

    # watch subcommand
    watch_p = sub.add_parser("watch", help="Watch a folder for new card images")
    watch_p.add_argument("folder", help="Folder to watch (CIFS share or local)")
    watch_p.add_argument("--db", default=None)
    watch_p.add_argument("--ingest", action="store_true",
                         help="Auto-POST matched cards to card-vault /api/ingest")
    watch_p.add_argument("--server", default="http://127.0.0.1:3000",
                         help="card-vault server URL (default: http://127.0.0.1:3000)")
    watch_p.add_argument("--condition", default="near_mint",
                         choices=["near_mint", "lightly_played", "moderately_played",
                                  "heavily_played", "damaged"],
                         help="Default condition for ingested cards")
    watch_p.add_argument("--threshold", type=int, default=MATCH_THRESHOLD)

    args = parser.parse_args()
    db_path = load_db_path(args.db)

    if not os.path.exists(db_path):
        print(f"ERROR: Database not found: {db_path}", file=sys.stderr)
        sys.exit(1)

    # Override global threshold if specified
    global MATCH_THRESHOLD
    MATCH_THRESHOLD = args.threshold

    index = HashIndex(db_path)
    if not index.hashes:
        print("WARNING: No hashes in database. Run 'python hash_cards.py' first.", file=sys.stderr)

    if args.command == "test":
        print(f"\nRecognizing: {args.image}")
        result = recognize_image(args.image, index, verbose=True)

        if result:
            print(f"\nResult:")
            print(f"  Name:      {result['name']}")
            print(f"  Set:       {result['set_name']} ({result['set_code']})")
            print(f"  Number:    {result['collector_number']}")
            print(f"  Rarity:    {result['rarity']}")
            print(f"  Scryfall:  {result['scryfall_id']}")
            print(f"  Distance:  {result['distance']}/64  ({'CONFIDENT' if result['confident'] else 'LOW CONFIDENCE'})")

            if args.save_art:
                # Re-process and save the art crop for visual inspection
                img_bgr = cv2.imread(args.image)
                corners = find_card_contour(img_bgr)
                if corners is not None:
                    warped = perspective_warp(img_bgr, corners)
                else:
                    h, w = img_bgr.shape[:2]
                    mx, my = int(w * 0.05), int(h * 0.05)
                    warped = cv2.resize(img_bgr[my:h-my, mx:w-mx], (CARD_W, CARD_H))
                art = crop_art_region(warped)
                out = "art_crop_debug.jpg"
                cv2.imwrite(out, art)
                cv2.imwrite("card_warped_debug.jpg", warped)
                print(f"\n  Saved art crop → {out}")
                print(f"  Saved warped card → card_warped_debug.jpg")
        else:
            print("  No result.")
            sys.exit(1)

    elif args.command == "watch":
        folder = args.folder
        if not os.path.isdir(folder):
            print(f"ERROR: Folder not found: {folder}", file=sys.stderr)
            sys.exit(1)

        print(f"Watching: {folder}")
        print(f"Server:   {args.server}")
        print(f"Ingest:   {'YES' if args.ingest else 'NO (--ingest to enable)'}")
        print(f"Condition:{args.condition}")
        print(f"Threshold:{MATCH_THRESHOLD}")
        print("\nReady. Waiting for images...\n")

        pair_tracker = CardCapturePair()
        handler = ScanHandler(
            index=index,
            pair_tracker=pair_tracker,
            server_url=args.server if args.ingest else None,
            auto_ingest=args.ingest,
            condition=args.condition,
        )

        observer = Observer()
        observer.schedule(handler, folder, recursive=False)
        observer.start()
        try:
            while True:
                time.sleep(1)
        except KeyboardInterrupt:
            print("\nStopping...")
        finally:
            observer.stop()
            observer.join()


if __name__ == "__main__":
    main()
