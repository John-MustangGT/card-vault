#!/usr/bin/env python3
"""
hash_cards.py — Pre-compute perceptual hashes for card art recognition.

Downloads art_crop images from Scryfall, crops the art region, computes a
64-bit dHash, and stores the result in card_hashes table.

Usage:
    python hash_cards.py                  # hash only cards in inventory_lots
    python hash_cards.py --all            # hash every card in scryfall_bulk_cards
    python hash_cards.py --set leg        # hash all cards from a specific set
    python hash_cards.py --limit 500      # cap at N cards (useful for testing)
    python hash_cards.py --threads 4      # parallel downloads (default: 2)
    python hash_cards.py --db ../card-vault.db

The script is resumable — cards already in card_hashes are skipped.
Rate limiting: 100ms between requests (Scryfall guideline: max 10 req/s).
"""

import argparse
import os
import re
import sqlite3
import sys
import time
import io
import hashlib
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
from threading import Semaphore, Lock

import requests
from PIL import Image
import imagehash
from dotenv import load_dotenv

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

# Art region as fraction of card image (portrait orientation)
# Scryfall art_crop already isolates the artwork, so we use a generous crop
# that excludes the card name banner at top and type/text box at bottom.
# Measured empirically from standard card layout.
ART_Y_TOP    = 0.04   # 4% from top — trim off the very top edge/border
ART_Y_BOTTOM = 0.96   # 96% from top — most of the art_crop image is art
ART_X_LEFT   = 0.04   # 4% from left — trim border
ART_X_RIGHT  = 0.96   # 96% from right — trim border

HASH_SIZE = 8          # 8×8 → 64-bit dHash
REQUEST_DELAY = 0.11   # seconds between HTTP requests (≤10/s per Scryfall TOS)
USER_AGENT = "card-vault/1.0 (collection manager; contact via github)"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def art_crop_url(image_uri: str) -> str:
    """Convert a Scryfall 'normal' image URI to the art_crop variant."""
    if not image_uri:
        return ""
    # Replace /normal/ with /art_crop/ in the path component
    url = re.sub(r'/normal/', '/art_crop/', image_uri)
    # Some URIs use a version query param — strip image size from filename too
    # e.g. en_us.jpg → en_us.jpg (art_crop has its own sizing)
    return url


def compute_dhash(img_bytes: bytes) -> str | None:
    """
    Given raw image bytes, crop to the art region and compute dHash.
    Returns a 16-char hex string (64-bit hash), or None on error.
    """
    try:
        img = Image.open(io.BytesIO(img_bytes)).convert("RGB")
        w, h = img.size
        left   = int(w * ART_X_LEFT)
        right  = int(w * ART_X_RIGHT)
        top    = int(h * ART_Y_TOP)
        bottom = int(h * ART_Y_BOTTOM)
        cropped = img.crop((left, top, right, bottom))
        h64 = imagehash.dhash(cropped, hash_size=HASH_SIZE)
        return str(h64)   # returns hex string like "f3a1b2c3d4e5f6a7"
    except Exception as e:
        print(f"  [dhash error] {e}", file=sys.stderr)
        return None


def fetch_image(url: str, session: requests.Session, rate_lock: Lock, last_req: list) -> bytes | None:
    """Download an image with rate limiting. Returns raw bytes or None."""
    with rate_lock:
        now = time.monotonic()
        elapsed = now - last_req[0]
        if elapsed < REQUEST_DELAY:
            time.sleep(REQUEST_DELAY - elapsed)
        last_req[0] = time.monotonic()

    try:
        resp = session.get(url, timeout=15)
        if resp.status_code == 200:
            return resp.content
        elif resp.status_code == 404:
            print(f"  [404] {url}", file=sys.stderr)
            return None
        elif resp.status_code == 429:
            print(f"  [429 rate limit] sleeping 5s...", file=sys.stderr)
            time.sleep(5)
            return None
        else:
            print(f"  [HTTP {resp.status_code}] {url}", file=sys.stderr)
            return None
    except Exception as e:
        print(f"  [fetch error] {url}: {e}", file=sys.stderr)
        return None


# ---------------------------------------------------------------------------
# Database helpers
# ---------------------------------------------------------------------------

def get_cards_to_hash(conn: sqlite3.Connection, mode: str, set_code: str | None, limit: int | None) -> list[tuple]:
    """
    Returns list of (scryfall_id, image_uri) tuples that don't yet have hashes.
    mode: 'inventory' | 'all'
    """
    already_hashed = """
        SELECT scryfall_id FROM card_hashes
    """

    if mode == "inventory":
        # Only cards that appear in inventory_lots (via scryfall_cards)
        base_sql = """
            SELECT DISTINCT sc.scryfall_id, sc.image_uri
            FROM scryfall_cards sc
            JOIN inventory_lots il ON il.scryfall_id = sc.scryfall_id
            WHERE sc.image_uri IS NOT NULL
              AND sc.scryfall_id NOT IN (SELECT scryfall_id FROM card_hashes)
        """
    else:
        # All cards in the bulk cache table
        base_sql = """
            SELECT DISTINCT scryfall_id, image_uri
            FROM scryfall_bulk_cards
            WHERE image_uri IS NOT NULL
              AND scryfall_id NOT IN (SELECT scryfall_id FROM card_hashes)
        """

    if set_code:
        if mode == "inventory":
            base_sql += f" AND sc.set_code = ?"
        else:
            base_sql += f" AND set_code = ?"

    base_sql += " ORDER BY scryfall_id"

    if limit:
        base_sql += f" LIMIT {int(limit)}"

    if set_code:
        rows = conn.execute(base_sql, (set_code.lower(),)).fetchall()
    else:
        rows = conn.execute(base_sql).fetchall()

    return rows


def save_hash(conn: sqlite3.Connection, scryfall_id: str, dhash: str, url: str):
    """Insert or replace a hash in card_hashes."""
    now = int(time.time())
    conn.execute(
        """
        INSERT OR REPLACE INTO card_hashes (scryfall_id, dhash, art_crop_url, computed_at)
        VALUES (?, ?, ?, ?)
        """,
        (scryfall_id, dhash, url, now),
    )


# ---------------------------------------------------------------------------
# Main processing
# ---------------------------------------------------------------------------

def process_card(args):
    """Worker function: fetch + hash one card. Returns (scryfall_id, dhash, url) or None."""
    scryfall_id, image_uri, session, rate_lock, last_req = args
    url = art_crop_url(image_uri)
    if not url:
        return None
    img_bytes = fetch_image(url, session, rate_lock, last_req)
    if not img_bytes:
        return None
    dhash = compute_dhash(img_bytes)
    if not dhash:
        return None
    return (scryfall_id, dhash, url)


def main():
    parser = argparse.ArgumentParser(description="Pre-compute card art hashes for recognition")
    parser.add_argument("--db", default=None, help="Path to card-vault.db (default: auto-detect from .env)")
    parser.add_argument("--all", action="store_true", help="Hash all cards in scryfall_bulk_cards (default: inventory only)")
    parser.add_argument("--set", dest="set_code", default=None, help="Only hash cards from this set code (e.g. 'leg')")
    parser.add_argument("--limit", type=int, default=None, help="Max number of cards to process")
    parser.add_argument("--threads", type=int, default=2, help="Parallel download threads (default: 2, max: 4)")
    args = parser.parse_args()

    # Locate database
    db_path = args.db
    if not db_path:
        # Try loading from .env in parent directory
        env_file = Path(__file__).parent.parent / ".env"
        if env_file.exists():
            load_dotenv(env_file)
        db_url = os.environ.get("DATABASE_URL", "sqlite:./card-vault.db")
        # Strip sqlite: prefix
        db_path = re.sub(r'^sqlite:', '', db_url).lstrip('/')
        # Resolve relative to repo root (one level up from scanner/)
        if not os.path.isabs(db_path):
            db_path = str(Path(__file__).parent.parent / db_path)

    if not os.path.exists(db_path):
        print(f"ERROR: Database not found at {db_path}", file=sys.stderr)
        print("Use --db /path/to/card-vault.db to specify location", file=sys.stderr)
        sys.exit(1)

    print(f"Database: {db_path}")

    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row

    # Ensure card_hashes table exists (in case migration hasn't been run)
    conn.execute("""
        CREATE TABLE IF NOT EXISTS card_hashes (
            scryfall_id     TEXT PRIMARY KEY,
            dhash           TEXT NOT NULL,
            art_crop_url    TEXT,
            computed_at     INTEGER NOT NULL
        )
    """)
    conn.commit()

    mode = "all" if args.all else "inventory"
    print(f"Mode: {mode}{' (set: ' + args.set_code + ')' if args.set_code else ''}")

    cards = get_cards_to_hash(conn, mode, args.set_code, args.limit)
    total = len(cards)
    print(f"Cards to hash: {total}")

    if total == 0:
        print("Nothing to do.")
        conn.close()
        return

    # HTTP session with proper headers
    session = requests.Session()
    session.headers.update({"User-Agent": USER_AGENT})

    threads = max(1, min(args.threads, 4))
    rate_lock = Lock()
    last_req = [0.0]   # mutable container for last request timestamp

    done = 0
    errors = 0
    commit_batch = []

    print(f"Starting with {threads} thread(s)...")
    t0 = time.monotonic()

    work_items = [
        (scryfall_id, image_uri, session, rate_lock, last_req)
        for scryfall_id, image_uri in cards
    ]

    with ThreadPoolExecutor(max_workers=threads) as executor:
        futures = {executor.submit(process_card, item): item[0] for item in work_items}
        for future in as_completed(futures):
            scryfall_id = futures[future]
            try:
                result = future.result()
            except Exception as e:
                print(f"  [exception] {scryfall_id}: {e}", file=sys.stderr)
                result = None

            if result:
                commit_batch.append(result)
                done += 1
            else:
                errors += 1

            # Commit every 50 rows
            if len(commit_batch) >= 50:
                for r in commit_batch:
                    save_hash(conn, *r)
                conn.commit()
                commit_batch.clear()

            # Progress
            n = done + errors
            if n % 100 == 0 or n == total:
                elapsed = time.monotonic() - t0
                rate = n / elapsed if elapsed > 0 else 0
                eta = (total - n) / rate if rate > 0 else 0
                print(f"  [{n}/{total}] {done} ok, {errors} errors — {rate:.1f}/s — ETA {eta:.0f}s")

    # Final commit
    for r in commit_batch:
        save_hash(conn, *r)
    conn.commit()

    elapsed = time.monotonic() - t0
    print(f"\nDone: {done} hashed, {errors} errors in {elapsed:.1f}s")

    # Summary
    total_hashed = conn.execute("SELECT COUNT(*) FROM card_hashes").fetchone()[0]
    print(f"Total in card_hashes table: {total_hashed}")

    conn.close()


if __name__ == "__main__":
    main()
