# Card Scanner

Perceptual-hash based card recognition for the Elmo OX-1 document camera (or FI-8040 when it arrives).

## How it works

1. **Pre-computation** (`hash_cards.py`): Downloads the `art_crop` version of each card's image from Scryfall, crops to the actual art region, computes a 64-bit dHash, and stores it in the `card_hashes` SQLite table. This only needs to run once (resumable).

2. **Recognition** (`recognize.py`):
   - Detects card boundary in the photo using OpenCV contour detection
   - Perspective-corrects to a standard 630×880 portrait
   - Crops the art region (same percentages as pre-computation)
   - Computes dHash and finds the closest Hamming-distance match in the index
   - Distance ≤ 12 (out of 64 bits) → confident match

3. **Ingest** (`POST /api/ingest`): On a match, the scanner posts the scryfall_id + condition + foil + scan images to card-vault, which creates an `individual_cards` entry, assigns the next UID from the pool, and saves the scans.

## Setup

```bat
cd scanner
setup.bat
```

Or manually:
```bat
python -m venv .venv
.venv\Scripts\activate
pip install -r requirements.txt
```

## Pre-computing hashes

Run this before using the scanner. It's resumable — run it again any time to pick up new cards.

```bash
# Only hash cards you own (faster, ~seconds for a typical collection)
python hash_cards.py

# Hash everything in the bulk data cache (~30,000+ cards, takes ~1 hour first run)
python hash_cards.py --all

# Hash a specific set only
python hash_cards.py --set leg

# Hash with more threads (faster downloads, but be respectful of Scryfall)
python hash_cards.py --threads 3

# Point at a specific database
python hash_cards.py --db C:\path\to\card-vault.db
```

## Testing recognition

```bash
# Test a single image — shows top 3 matches + distance
python recognize.py test photo.jpg

# Test and save debug images (art_crop_debug.jpg, card_warped_debug.jpg)
python recognize.py test photo.jpg --save-art
```

### Interpreting results

| Distance | Meaning |
|---|---|
| 0–6 | Near-perfect match |
| 7–12 | Good match (default threshold) |
| 13–20 | Uncertain — check visually |
| 21+ | No match |

If you're getting too many false positives, raise `--threshold` to 10 or 8.
If confident matches are being missed, raise it to 15.

## Elmo OX-1 — Live Camera Mode (recommended)

`capture.py` connects directly to the OX-1 over USB (UVC) — no external software needed, no folder watching, fully hands-free.

### Quick start

```bash
# 1. Find the device index
python capture.py --list-cameras
#   [0] 1920×1080 @ 30fps   ← built-in webcam
#   [1] 1920×1080 @ 30fps   ← Elmo OX-1

# 2. Preview only (no ingest) — good for testing recognition
python capture.py --camera 1

# 3. Full live ingest
python capture.py --camera 1 --ingest --condition near_mint
```

### The workflow
1. card-vault is running (`cargo run`)
2. `python capture.py --camera 1 --ingest` is running in another terminal
3. A preview window opens showing the camera feed
4. **Place a card face-up** on a dark matte background under the OX-1
5. The script detects when the frame stabilizes (~0.6s), auto-captures, recognizes the card, and beeps
6. The result banner appears at the bottom of the preview
7. **Flip the card face-down** — script captures the back and beeps twice
8. The pair is ingested: `individual_cards` entry created, next UID assigned, both scans saved
9. **Remove the card** — the script resets and is ready for the next one

Default mode ingests on the front alone (back is optional). Use `--pair` to require both before ingesting.

### Keyboard shortcuts

| Key | Action |
|---|---|
| `SPACE` | Force capture immediately |
| `C` | Cycle condition (NM → LP → MP → HP → DMG) |
| `F` | Cycle foil (normal → foil → etched) |
| `R` | Reload hash index from database |
| `Q` / `ESC` | Quit |

### Tuning

```bash
# If cards aren't triggering auto-capture (too much vibration)
python capture.py --camera 1 --stable-diff 4.0 --stable-frames 25

# If it captures too eagerly
python capture.py --camera 1 --stable-diff 1.5 --stable-frames 12

# Looser recognition threshold (more matches, more false positives)
python capture.py --camera 1 --threshold 16

# Camera feed upside-down
python capture.py --camera 1 --flip 180
```

### Overlay guide

- **Gray border** — waiting for card
- **Yellow border** — card detected, stabilizing (progress bar at bottom)
- **Green border** — card captured / recognized
- **Bottom banner** — card name, set, collector number, match distance
- **Top-left panel** — current condition/foil, cards ingested this session

## Elmo OX-1 — Watch Folder Mode (fallback)

If you prefer using the OX-1's own capture software to save images:

1. Configure OX-1 software to save images to a folder (e.g. `C:\OX1-Output`)
2. Run in watch mode:
   ```bash
   python recognize.py watch C:\OX1-Output --ingest --condition near_mint
   ```
3. The script will print the matched card name/set for each image

### Front + Back pairing (watch mode)
The watcher pairs consecutive images taken within 10 seconds as front/back of the same card. Place front → OX-1 saves front → flip card → OX-1 saves back → script pairs them automatically.

## FI-8040 (future)
The FI-8040 duplex scanner can capture front+back in a single pass. Configure it to drop JPEG pairs to a CIFS/SMB share, then point `recognize.py watch` at that share. The script handles the pairing automatically via the 10-second window.

## Troubleshooting

**"No hashes in database"** — Run `hash_cards.py` first.

**"No clean contour — using full image"** — The card boundary detection failed (low contrast, bad lighting, cluttered background). Use a dark matte background for best results. The fallback center-crop still works reasonably well.

**Low confidence matches on old cards** — Alpha/Beta/Unlimited cards have the same art as later printings. The system will match to whichever printing is in your hash index. Manually confirm edition after the fact.

**"UID pool is empty"** — Go to `/labels` in card-vault and generate more UIDs before scanning.
