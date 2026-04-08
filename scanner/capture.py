#!/usr/bin/env python3
"""
capture.py — Live camera capture for the Elmo OX-1 document camera.

Connects directly to the camera via OpenCV (UVC/DirectShow), shows a live
preview, and auto-captures when the frame stabilizes (card placed, hand
removed). Runs recognition and optionally posts to /api/ingest.

Usage:
    python capture.py                         # auto-detect camera, preview only
    python capture.py --ingest                # auto-ingest matched cards
    python capture.py --camera 1              # use specific device index
    python capture.py --list-cameras          # show all available cameras
    python capture.py --flip 180              # rotate feed (0, 90, 180, 270)
    python capture.py --condition lightly_played --foil foil
    python capture.py --threshold 15          # looser match threshold

Keyboard shortcuts (preview window):
    SPACE   Force capture immediately (bypass stability detection)
    C       Change condition for next card
    F       Toggle foil type for next card
    R       Reload hash index from database
    Q / ESC Quit

Front + Back workflow:
    Place card face-up → auto-captures front → overlay shows "Flip card"
    Flip card face-down → auto-captures back → pair ingested together
"""

import argparse
import platform
import os
import re
import sys
import time
import io
import json
import sqlite3
import tempfile
import threading
from pathlib import Path

import cv2
import numpy as np
import requests
from PIL import Image
import imagehash
from dotenv import load_dotenv

# Try winsound for audio feedback (Windows only); silent fallback otherwise
try:
    import winsound
    def beep(freq: int = 1000, duration_ms: int = 120):
        winsound.Beep(freq, duration_ms)
    def beep_error():
        winsound.Beep(300, 300)
except ImportError:
    def beep(freq: int = 1000, duration_ms: int = 120):
        print("\a", end="", flush=True)
    def beep_error():
        print("\a\a", end="", flush=True)

# Re-use helpers from recognize.py
sys.path.insert(0, str(Path(__file__).parent))
from recognize import (
    HashIndex, recognize_image, find_card_contour, perspective_warp,
    crop_art_region, compute_dhash_from_array, MATCH_THRESHOLD,
    CARD_W, CARD_H, post_ingest, CardCapturePair,
)

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

# Frame stability detection
STABLE_DIFF_THRESHOLD = 2.5    # mean per-pixel diff to count as "stable"
STABLE_FRAMES_NEEDED  = 18     # consecutive stable frames before capture (~0.6s at 30fps)
MOTION_DIFF_THRESHOLD = 8.0    # mean diff to detect "card removed"
MOTION_FRAMES_NEEDED  = 6      # consecutive motion frames to reset state

# Preview window
PREVIEW_W = 1280
PREVIEW_H = 720

# Colors (BGR)
COLOR_WHITE   = (255, 255, 255)
COLOR_GREEN   = (0, 220, 60)
COLOR_YELLOW  = (0, 200, 220)
COLOR_RED     = (50, 50, 220)
COLOR_BLUE    = (220, 160, 60)
COLOR_GRAY    = (120, 120, 120)
COLOR_BLACK   = (0, 0, 0)
COLOR_AMBER   = (0, 165, 255)

FONT      = cv2.FONT_HERSHEY_SIMPLEX
FONT_MONO = cv2.FONT_HERSHEY_PLAIN

# Capture state machine
STATE_WAITING     = "waiting"     # no card present
STATE_STABILIZING = "stabilizing" # card detected, counting stable frames
STATE_CAPTURED    = "captured"    # front captured, showing result
STATE_PAIRING     = "pairing"     # waiting for back
STATE_DONE        = "done"        # pair complete, waiting for card removal

# ---------------------------------------------------------------------------
# Camera enumeration
# ---------------------------------------------------------------------------

def _cam_backend() -> int:
    """Return the best OpenCV camera backend for the current platform."""
    if platform.system() == "Windows":
        return cv2.CAP_DSHOW
    elif platform.system() == "Linux":
        return cv2.CAP_V4L2
    return cv2.CAP_ANY


def _v4l2_devices() -> list[str]:
    """Return /dev/video* device paths present on Linux."""
    import glob
    return sorted(glob.glob("/dev/video*"))


def list_cameras(max_index: int = 10) -> list[dict]:
    """Probe device indices 0..max_index-1 and return info on working ones."""
    found = []
    backend = _cam_backend()

    if platform.system() == "Linux":
        devs = _v4l2_devices()
        print(f"V4L2 devices: {devs if devs else 'none found'}")
        if not devs:
            print("  OX-1 connected? Check: lsusb  and  ls /dev/video*")
            print("  May need: sudo apt install v4l-utils  then  v4l2-ctl --list-devices")

    print("Scanning for cameras...")
    for i in range(max_index):
        cap = cv2.VideoCapture(i, backend)
        if cap.isOpened():
            ret, _ = cap.read()
            if ret:
                w   = int(cap.get(cv2.CAP_PROP_FRAME_WIDTH))
                h   = int(cap.get(cv2.CAP_PROP_FRAME_HEIGHT))
                fps = cap.get(cv2.CAP_PROP_FPS)
                found.append({"index": i, "width": w, "height": h, "fps": fps})
                print(f"  [{i}] {w}×{h} @ {fps:.0f}fps")
            cap.release()
        else:
            cap.release()
    if not found:
        print("  No cameras found.")
        if platform.system() == "Linux":
            print("  Try: sudo usermod -aG video $USER  then log out and back in")
    return found


def open_camera(index: int, width: int = 0, height: int = 0) -> cv2.VideoCapture:
    """Open a camera by index using the platform-appropriate backend."""
    backend = _cam_backend()
    cap = cv2.VideoCapture(index, backend)
    if not cap.isOpened():
        # Fallback: let OpenCV pick
        cap = cv2.VideoCapture(index)
    if not cap.isOpened():
        raise RuntimeError(f"Cannot open camera index {index}")
    # Request higher resolution if camera supports it
    if width and height:
        cap.set(cv2.CAP_PROP_FRAME_WIDTH,  width)
        cap.set(cv2.CAP_PROP_FRAME_HEIGHT, height)
    # Buffer size: keep small so we always get the latest frame
    cap.set(cv2.CAP_PROP_BUFFERSIZE, 1)
    return cap


def rotate_frame(frame: np.ndarray, degrees: int) -> np.ndarray:
    if degrees == 90:
        return cv2.rotate(frame, cv2.ROTATE_90_CLOCKWISE)
    elif degrees == 180:
        return cv2.rotate(frame, cv2.ROTATE_180)
    elif degrees == 270:
        return cv2.rotate(frame, cv2.ROTATE_90_COUNTERCLOCKWISE)
    return frame


# ---------------------------------------------------------------------------
# Overlay drawing helpers
# ---------------------------------------------------------------------------

def draw_text_shadow(img, text, pos, font, scale, color, thickness=1):
    """Draw text with a dark drop shadow for legibility on any background."""
    x, y = pos
    cv2.putText(img, text, (x+1, y+1), font, scale, COLOR_BLACK, thickness + 1, cv2.LINE_AA)
    cv2.putText(img, text, (x,   y  ), font, scale, color, thickness, cv2.LINE_AA)


def draw_border(img, color, thickness=6):
    h, w = img.shape[:2]
    cv2.rectangle(img, (0, 0), (w - 1, h - 1), color, thickness)


def draw_progress_bar(img, fraction: float, y: int, color):
    """Horizontal progress bar across the bottom."""
    h, w = img.shape[:2]
    bar_w = int(w * fraction)
    cv2.rectangle(img, (0, y), (bar_w, y + 8), color, -1)
    cv2.rectangle(img, (0, y), (w,     y + 8), COLOR_GRAY, 1)


def draw_status_panel(img, lines: list[tuple[str, tuple]], y_start: int = 20):
    """
    Draw a semi-transparent info panel in the top-left.
    lines: list of (text, color)
    """
    if not lines:
        return
    padding = 8
    line_h = 22
    max_w = max(cv2.getTextSize(t, FONT, 0.55, 1)[0][0] for t, _ in lines) + padding * 2
    panel_h = len(lines) * line_h + padding * 2
    overlay = img.copy()
    cv2.rectangle(overlay, (10, y_start), (10 + max_w, y_start + panel_h), (20, 20, 20), -1)
    cv2.addWeighted(overlay, 0.6, img, 0.4, 0, img)
    for i, (text, color) in enumerate(lines):
        y = y_start + padding + (i + 1) * line_h - 4
        draw_text_shadow(img, text, (10 + padding, y), FONT, 0.55, color)


def draw_result_banner(img, result: dict):
    """Large result overlay at the bottom of the frame."""
    h, w = img.shape[:2]
    banner_h = 90
    overlay = img.copy()
    cv2.rectangle(overlay, (0, h - banner_h), (w, h), (10, 10, 10), -1)
    cv2.addWeighted(overlay, 0.75, img, 0.25, 0, img)

    name = result.get("name", "Unknown")
    set_info = f"{result.get('set_name', '')}  ({result.get('set_code', '').upper()} #{result.get('collector_number', '')})"
    dist = result.get("distance", 99)
    confident = result.get("confident", False)
    conf_label = f"dist={dist}  {'✓ MATCH' if confident else '? LOW CONF'}"
    conf_color = COLOR_GREEN if confident else COLOR_AMBER

    draw_text_shadow(img, name,      (16, h - banner_h + 28), FONT, 0.9, COLOR_WHITE, 2)
    draw_text_shadow(img, set_info,  (16, h - banner_h + 56), FONT, 0.6, COLOR_GRAY)
    draw_text_shadow(img, conf_label,(16, h - banner_h + 78), FONT, 0.55, conf_color)


# ---------------------------------------------------------------------------
# Session state
# ---------------------------------------------------------------------------

CONDITION_CYCLE = ["near_mint", "lightly_played", "moderately_played", "heavily_played", "damaged"]
CONDITION_SHORT = {"near_mint": "NM", "lightly_played": "LP", "moderately_played": "MP",
                   "heavily_played": "HP", "damaged": "DMG"}
FOIL_CYCLE = ["normal", "foil", "etched"]


class SessionState:
    def __init__(self, condition: str, foil: str):
        self.condition = condition
        self.foil = foil
        self.state = STATE_WAITING
        self.stable_count = 0
        self.motion_count = 0
        self.last_result: dict | None = None
        self.front_frame: np.ndarray | None = None
        self.front_result: dict | None = None
        self.front_time: float = 0.0
        self.ingest_result: dict | None = None
        self.cards_ingested = 0
        self.session_start = time.monotonic()

    def cycle_condition(self):
        idx = CONDITION_CYCLE.index(self.condition)
        self.condition = CONDITION_CYCLE[(idx + 1) % len(CONDITION_CYCLE)]

    def cycle_foil(self):
        idx = FOIL_CYCLE.index(self.foil)
        self.foil = FOIL_CYCLE[(idx + 1) % len(FOIL_CYCLE)]


# ---------------------------------------------------------------------------
# Capture + recognition
# ---------------------------------------------------------------------------

def frame_diff(a: np.ndarray, b: np.ndarray) -> float:
    """Mean per-pixel absolute difference between two frames (grayscale)."""
    ga = cv2.cvtColor(a, cv2.COLOR_BGR2GRAY)
    gb = cv2.cvtColor(b, cv2.COLOR_BGR2GRAY)
    return float(np.mean(cv2.absdiff(ga, gb)))


def capture_and_recognize(frame: np.ndarray, index: HashIndex) -> dict | None:
    """
    Run card recognition on a single frame.
    Returns result dict or None.
    """
    # Save to a temp file so we can reuse recognize_image() cleanly
    with tempfile.NamedTemporaryFile(suffix=".jpg", delete=False) as tmp:
        tmp_path = tmp.name
    try:
        cv2.imwrite(tmp_path, frame)
        result = recognize_image(tmp_path, index, verbose=False)
        return result
    finally:
        try:
            os.unlink(tmp_path)
        except OSError:
            pass


def save_frame_to_temp(frame: np.ndarray) -> str:
    """Save a frame to a temp JPEG and return the path."""
    with tempfile.NamedTemporaryFile(suffix=".jpg", delete=False) as tmp:
        path = tmp.name
    cv2.imwrite(path, frame)
    return path


def ingest_async(server_url: str, scryfall_id: str, condition: str, foil: str,
                 front_path: str, back_path: str | None,
                 callback):
    """Post ingest in a background thread; call callback(result) when done."""
    def _run():
        result = post_ingest(server_url, scryfall_id, condition, foil, front_path, back_path)
        callback(result)
        # Clean up temp files
        for p in [front_path, back_path]:
            if p and os.path.exists(p):
                try:
                    os.unlink(p)
                except OSError:
                    pass
    threading.Thread(target=_run, daemon=True).start()


# ---------------------------------------------------------------------------
# Main loop
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


def run_capture(args):
    db_path = load_db_path(args.db)
    if not os.path.exists(db_path):
        print(f"ERROR: Database not found: {db_path}", file=sys.stderr)
        sys.exit(1)

    print(f"Database: {db_path}")
    index = HashIndex(db_path)
    if not index.hashes:
        print("WARNING: No hashes loaded. Run 'python hash_cards.py' first.", file=sys.stderr)

    # Open camera
    cam_index = args.camera
    print(f"Opening camera [{cam_index}]...")
    try:
        cap = open_camera(cam_index, width=1920, height=1080)
    except RuntimeError as e:
        print(f"ERROR: {e}")
        print("Try --list-cameras to see available devices, or --camera N to select one.")
        sys.exit(1)

    actual_w = int(cap.get(cv2.CAP_PROP_FRAME_WIDTH))
    actual_h = int(cap.get(cv2.CAP_PROP_FRAME_HEIGHT))
    actual_fps = cap.get(cv2.CAP_PROP_FPS)
    print(f"Camera opened: {actual_w}×{actual_h} @ {actual_fps:.0f}fps")
    print(f"Ingest: {'YES → ' + args.server if args.ingest else 'NO (--ingest to enable)'}")
    print()
    print("Keys: SPACE=capture  C=condition  F=foil  R=reload hashes  Q/ESC=quit")
    print()

    sess = SessionState(args.condition, args.foil)
    prev_gray_frame: np.ndarray | None = None

    cv2.namedWindow("Card Vault Scanner", cv2.WINDOW_NORMAL)
    cv2.resizeWindow("Card Vault Scanner", PREVIEW_W, PREVIEW_H)

    while True:
        ret, raw_frame = cap.read()
        if not ret:
            print("Camera read failed — exiting.")
            break

        frame = rotate_frame(raw_frame, args.flip)

        # ── Stability detection ──────────────────────────────────────────
        force_capture = False
        diff = 0.0
        if prev_gray_frame is not None:
            diff = frame_diff(prev_gray_frame, frame)

            if sess.state == STATE_WAITING:
                if diff < STABLE_DIFF_THRESHOLD:
                    sess.stable_count += 1
                    if sess.stable_count >= STABLE_FRAMES_NEEDED:
                        sess.state = STATE_STABILIZING
                        sess.stable_count = 0
                else:
                    sess.stable_count = 0

            elif sess.state == STATE_STABILIZING:
                if diff < STABLE_DIFF_THRESHOLD:
                    sess.stable_count += 1
                    if sess.stable_count >= STABLE_FRAMES_NEEDED // 2:
                        force_capture = True
                else:
                    # Motion resumed — back to waiting
                    sess.stable_count = 0
                    sess.state = STATE_WAITING

            elif sess.state in (STATE_CAPTURED, STATE_PAIRING, STATE_DONE):
                if diff > MOTION_DIFF_THRESHOLD:
                    sess.motion_count += 1
                    if sess.motion_count >= MOTION_FRAMES_NEEDED:
                        # Card removed
                        if sess.state == STATE_DONE:
                            print("  Card removed — ready for next")
                        elif sess.state == STATE_CAPTURED:
                            print("  Card removed — front only (no back captured)")
                        sess.state = STATE_WAITING
                        sess.stable_count = 0
                        sess.motion_count = 0
                        sess.last_result = None
                        sess.front_result = None
                        sess.front_frame = None
                        sess.ingest_result = None
                else:
                    sess.motion_count = 0

        prev_gray_frame = frame.copy()

        # ── Keyboard input ───────────────────────────────────────────────
        key = cv2.waitKey(1) & 0xFF
        if key in (ord('q'), ord('Q'), 27):  # Q or ESC
            break
        elif key == ord(' '):
            force_capture = True
        elif key in (ord('c'), ord('C')):
            sess.cycle_condition()
            print(f"  Condition → {sess.condition}")
        elif key in (ord('f'), ord('F')):
            sess.cycle_foil()
            print(f"  Foil → {sess.foil}")
        elif key in (ord('r'), ord('R')):
            print("  Reloading hash index...")
            index.load()

        # ── Capture trigger ──────────────────────────────────────────────
        if force_capture and sess.state in (STATE_WAITING, STATE_STABILIZING, STATE_CAPTURED):
            sess.state = STATE_WAITING   # reset so we don't re-trigger
            sess.stable_count = 0

            print(f"\n{'─'*50}")
            if sess.front_result is None:
                print("Capturing FRONT...")
            else:
                print("Capturing BACK...")

            result = capture_and_recognize(frame, index)

            if result is None:
                print("  Recognition failed.")
                beep_error()
            else:
                sess.last_result = result
                if result["confident"]:
                    print(f"  ✓ {result['name']} [{result['set_code']}]  dist={result['distance']}")
                    beep(1200, 100)
                else:
                    print(f"  ⚠ Low confidence: {result['name']} [{result['set_code']}]  dist={result['distance']}")
                    beep(600, 200)

                if sess.front_result is None:
                    # Store as front
                    sess.front_result = result
                    sess.front_frame = frame.copy()
                    sess.front_time = time.monotonic()
                    sess.state = STATE_CAPTURED
                    print("  Front stored. Flip card for back, or place next card.")

                    if not args.pair:
                        # Single-side mode: ingest immediately
                        if args.ingest and result["confident"]:
                            front_path = save_frame_to_temp(frame)

                            def on_done(r, _sess=sess):
                                _sess.ingest_result = r
                                if r and r.get("ok"):
                                    _sess.cards_ingested += 1
                                    print(f"  ✓ Ingested: {r.get('name')} → ID {r.get('card_id')}")
                                    beep(1000, 80)
                                    time.sleep(0.05)
                                    beep(1200, 80)
                                else:
                                    print(f"  ✗ Ingest failed: {r}")
                                    beep_error()
                                _sess.state = STATE_DONE

                            ingest_async(args.server, result["scryfall_id"],
                                         sess.condition, sess.foil,
                                         front_path, None, on_done)
                        else:
                            sess.state = STATE_DONE

                else:
                    # Back captured — form pair
                    front = sess.front_result
                    sess.state = STATE_DONE

                    if args.ingest and front["confident"]:
                        front_path = save_frame_to_temp(sess.front_frame)
                        back_path  = save_frame_to_temp(frame)

                        def on_done_pair(r, _sess=sess):
                            _sess.ingest_result = r
                            if r and r.get("ok"):
                                _sess.cards_ingested += 1
                                print(f"  ✓ Ingested (front+back): {r.get('name')} → ID {r.get('card_id')}")
                                beep(1000, 80)
                                time.sleep(0.05)
                                beep(1400, 80)
                                time.sleep(0.05)
                                beep(1600, 80)
                            else:
                                print(f"  ✗ Ingest failed: {r}")
                                beep_error()

                        ingest_async(args.server, front["scryfall_id"],
                                     sess.condition, sess.foil,
                                     front_path, back_path, on_done_pair)
                    else:
                        print(f"  Pair: {front['name']} / {result['name']}")

                    sess.front_result = None
                    sess.front_frame = None

        # ── Build display frame ──────────────────────────────────────────
        display = frame.copy()

        # Border color + stability bar
        if sess.state == STATE_WAITING:
            border_color = COLOR_GRAY
            progress = min(sess.stable_count / max(STABLE_FRAMES_NEEDED, 1), 1.0)
            if progress > 0:
                draw_progress_bar(display, progress, display.shape[0] - 12, COLOR_YELLOW)
        elif sess.state == STATE_STABILIZING:
            border_color = COLOR_YELLOW
            progress = min(sess.stable_count / max(STABLE_FRAMES_NEEDED // 2, 1), 1.0)
            draw_progress_bar(display, progress, display.shape[0] - 12, COLOR_GREEN)
        elif sess.state == STATE_CAPTURED:
            border_color = COLOR_GREEN
        elif sess.state == STATE_PAIRING:
            border_color = COLOR_BLUE
        elif sess.state == STATE_DONE:
            border_color = COLOR_GREEN

        draw_border(display, border_color, thickness=6)

        # Status panel (top-left)
        elapsed = time.monotonic() - sess.session_start
        cond_short = CONDITION_SHORT.get(sess.condition, sess.condition)
        status_lines = [
            (f"Condition: {cond_short}",  COLOR_WHITE),
            (f"Foil:      {sess.foil}",   COLOR_WHITE),
            (f"Ingested:  {sess.cards_ingested}", COLOR_GREEN if sess.cards_ingested else COLOR_GRAY),
            (f"Hashes:    {len(index.hashes)}", COLOR_GRAY),
            (f"diff={diff:.1f}", COLOR_GRAY),
        ]
        draw_status_panel(display, status_lines)

        # State message (top-center)
        state_msgs = {
            STATE_WAITING:     ("Place card face-up",    COLOR_GRAY),
            STATE_STABILIZING: ("Hold steady...",        COLOR_YELLOW),
            STATE_CAPTURED:    ("Flip for back  or  place next card",  COLOR_GREEN),
            STATE_PAIRING:     ("Scanning back...",      COLOR_BLUE),
            STATE_DONE:        ("Remove card",           COLOR_GREEN),
        }
        msg, msg_color = state_msgs.get(sess.state, ("", COLOR_WHITE))
        if msg:
            h, w = display.shape[:2]
            (tw, _), _ = cv2.getTextSize(msg, FONT, 0.75, 2)
            draw_text_shadow(display, msg, ((w - tw) // 2, 40), FONT, 0.75, msg_color, 2)

        # Result banner (bottom)
        result_to_show = sess.last_result
        if result_to_show:
            draw_result_banner(display, result_to_show)

            # Ingest result
            if sess.ingest_result:
                h, w = display.shape[:2]
                ir = sess.ingest_result
                if ir.get("ok"):
                    tag = f"✓ ID: {ir.get('card_id')}  {ir.get('name')} [{ir.get('set_code')}]"
                    draw_text_shadow(display, tag, (16, h - 100), FONT, 0.6, COLOR_GREEN, 1)
                else:
                    draw_text_shadow(display, "✗ Ingest failed", (16, h - 100), FONT, 0.6, COLOR_RED, 1)

        # Key hints (bottom-right)
        h, w = display.shape[:2]
        hints = "SPACE=capture  C=condition  F=foil  R=reload  Q=quit"
        (hw, _), _ = cv2.getTextSize(hints, FONT, 0.42, 1)
        draw_text_shadow(display, hints, (w - hw - 10, h - 10), FONT, 0.42, COLOR_GRAY)

        cv2.imshow("Card Vault Scanner", display)

    cap.release()
    cv2.destroyAllWindows()
    elapsed = time.monotonic() - sess.session_start
    print(f"\nSession complete: {sess.cards_ingested} cards ingested in {elapsed:.0f}s")


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main():
    global STABLE_FRAMES_NEEDED, STABLE_DIFF_THRESHOLD

    parser = argparse.ArgumentParser(
        description="Live camera capture for Elmo OX-1 / card-vault",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument("--list-cameras", action="store_true",
                        help="List available camera devices and exit")
    parser.add_argument("--camera", type=int, default=0,
                        help="Camera device index (default: 0)")
    parser.add_argument("--flip", type=int, default=0, choices=[0, 90, 180, 270],
                        help="Rotate camera feed (default: 0)")
    parser.add_argument("--db", default=None,
                        help="Path to card-vault.db")
    parser.add_argument("--ingest", action="store_true",
                        help="Auto-POST matched cards to /api/ingest")
    parser.add_argument("--server", default="http://127.0.0.1:3000",
                        help="card-vault server URL (default: http://127.0.0.1:3000)")
    parser.add_argument("--condition", default="near_mint",
                        choices=["near_mint", "lightly_played", "moderately_played",
                                 "heavily_played", "damaged"],
                        help="Starting condition (default: near_mint)")
    parser.add_argument("--foil", default="normal",
                        choices=["normal", "foil", "etched"],
                        help="Starting foil type (default: normal)")
    parser.add_argument("--pair", action="store_true",
                        help="Wait for front+back pair before ingesting (default: ingest on front only)")
    parser.add_argument("--threshold", type=int, default=MATCH_THRESHOLD,
                        help=f"Hamming distance match threshold (default: {MATCH_THRESHOLD})")
    parser.add_argument("--stable-frames", type=int, default=STABLE_FRAMES_NEEDED,
                        help=f"Frames needed for stability trigger (default: {STABLE_FRAMES_NEEDED})")
    parser.add_argument("--stable-diff", type=float, default=STABLE_DIFF_THRESHOLD,
                        help=f"Max frame diff to count as stable (default: {STABLE_DIFF_THRESHOLD})")

    args = parser.parse_args()

    if args.list_cameras:
        list_cameras()
        return

    # Apply tuning overrides
    import recognize as rec_mod
    rec_mod.MATCH_THRESHOLD = args.threshold
    STABLE_FRAMES_NEEDED  = args.stable_frames
    STABLE_DIFF_THRESHOLD = args.stable_diff

    run_capture(args)


if __name__ == "__main__":
    main()
