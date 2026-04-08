# card-vault

Local MTG card inventory and asset tracking system. Focused on asset management and sales rather than deck management.

**Stack:** Rust + axum · SQLite + sqlx · minijinja · Vanilla JS
**Scanner:** Python + OpenCV · perceptual hashing · Elmo OX-1 / FI-8040

---

## Prerequisites

### Rust (server)

**Windows / macOS / Linux:**
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# Then restart your shell, or run:
source "$HOME/.cargo/env"
```
On Windows you can also use [rustup-init.exe](https://rustup.rs).

Minimum version: **Rust 1.75+** (uses async traits)

### sqlx-cli (database migrations)

```bash
cargo install sqlx-cli --no-default-features --features sqlite
```

### Python 3.10+ (scanner — optional)

Only needed if you are using the card scanner feature.

**Windows:** [python.org](https://www.python.org/downloads/) — check "Add to PATH" during install
**Linux (Debian/Ubuntu):**
```bash
sudo apt install python3 python3-venv python3-pip
```
**macOS:**
```bash
brew install python
```

---

## First-time setup

### 1. Clone the repo

```bash
git clone <repo-url> card-vault
cd card-vault
```

### 2. Configure environment

```bash
cp .env .env.local     # keep a local copy if you want to customise
```

The defaults work out of the box. Edit `.env` if you need to change paths or the listen address:

| Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | `sqlite:./card-vault.db` | SQLite file — use an absolute path in production |
| `SCAN_STORAGE_PATH` | `./scans` | Front/back card scan images |
| `DATA_DIR` | `./data` | Scryfall bulk data `.json.gz` files |
| `HOST` | `localhost` | `localhost` = this machine only · `localnet` = LAN · `any` = all interfaces |
| `PORT` | `3000` | Listen port |

### 3. Create the database and run migrations

```bash
sqlx db create        # creates card-vault.db
sqlx migrate run      # applies all migrations in migrations/
```

### 4. Build and run

```bash
cargo run
# Server listening at http://127.0.0.1:3000
```

For a faster optimised build:
```bash
cargo run --release
```

---

## Linux-specific notes

### Dependencies for OpenCV (scanner)

If you plan to use the card scanner, OpenCV needs a few system libraries:

**Debian / Ubuntu:**
```bash
sudo apt install -y \
    libopencv-dev \
    libglib2.0-0 \
    libsm6 \
    libxext6 \
    libxrender-dev \
    libgl1-mesa-glx
```

**Fedora / RHEL:**
```bash
sudo dnf install -y opencv-devel mesa-libGL
```

### Running as a systemd service

Create `/etc/systemd/system/card-vault.service`:

```ini
[Unit]
Description=card-vault MTG inventory
After=network.target

[Service]
Type=simple
User=<your-user>
WorkingDirectory=/home/<your-user>/card-vault
EnvironmentFile=/home/<your-user>/card-vault/.env
ExecStart=/home/<your-user>/card-vault/target/release/card-vault
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```bash
cargo build --release
sudo systemctl daemon-reload
sudo systemctl enable --now card-vault
sudo systemctl status card-vault
```

To make it accessible on your LAN, set `HOST=localnet` in `.env` and open the port:
```bash
sudo ufw allow 3000/tcp    # Ubuntu/Debian
sudo firewall-cmd --add-port=3000/tcp --permanent && sudo firewall-cmd --reload   # Fedora/RHEL
```

---

## Loading card data

Card data comes from Scryfall's bulk data files. Download and import via the built-in market page:

1. Go to **Market → Import** in the UI (`/market`)
2. Click **Download Bulk Data** — fetches the latest `all-cards` file from Scryfall (~250 MB compressed) and stores it in `DATA_DIR`
3. Click **Import** to load it into `scryfall_bulk_cards`

Or via curl (the download can take a minute):
```bash
curl http://127.0.0.1:3000/market/import -X POST
```

You only need to do this once, then periodically to pick up new printings.

---

## Importing your collection

The import page (`/import`) accepts CSV exports from:

- **ManaBox** — export from the app, import as-is
- **CardSphere** — export from your collection, import as-is

Both formats are auto-detected from the header row.

---

## Scanner setup (Elmo OX-1)

The scanner uses perceptual hashing to identify cards from a live camera feed.

### 1. Install Python dependencies

**Windows:**
```bat
cd scanner
setup.bat
```

**Linux / macOS:**
```bash
cd scanner
python3 -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
```

### 2. Pre-compute card hashes

Downloads `art_crop` images from Scryfall and builds the recognition index. Only cards in your inventory are hashed by default (~seconds). Use `--all` to hash the full card database (~1 hour, resumable).

```bash
# Activate venv first (Linux/macOS: source .venv/bin/activate)
python hash_cards.py              # inventory cards only (recommended first run)
python hash_cards.py --all        # everything in the bulk cache
python hash_cards.py --set leg    # single set (e.g. Legends)
```

### 3. Find your camera index

```bash
python capture.py --list-cameras
# [0] 1920×1080 @ 30fps  ← built-in webcam
# [1] 1920×1080 @ 30fps  ← Elmo OX-1
```

### 4. Run the scanner

```bash
# Preview only — test recognition without ingesting
python capture.py --camera 1

# Full pipeline — auto-creates individual_cards entries
python capture.py --camera 1 --ingest --condition near_mint
```

Place a card face-up under the OX-1. The script detects when the frame stabilises, recognises the card, beeps, and creates the inventory entry automatically. Flip the card for a back scan, then remove it to scan the next one.

See [`scanner/SCANNER.md`](scanner/SCANNER.md) for full documentation including tuning, front/back pairing, and the FI-8040 watch-folder workflow.

---

## Accessing the UI

| Page | URL | Description |
|---|---|---|
| Inventory | `/inventory` | Browse, search, edit lots |
| Individuals | `/inventory/card/:id/individual` | Tracked singles, scans |
| Import | `/import` | ManaBox / CardSphere CSV import |
| Market | `/market` | Scryfall price browser |
| Ledger | `/ledger` | Sales P&L |
| Sales | `/sales` | Create invoices |
| Locations | `/locations` | Storage binders/boxes |
| Labels | `/labels` | UID pool + Avery 8167 label printing |

---

## Development notes

### Running migrations after schema changes

```bash
sqlx migrate run
```

### Checking queries compile (sqlx offline mode)

```bash
cargo sqlx prepare
```

### Project layout

```
migrations/          SQL migrations — source of truth for schema
src/
  main.rs            Server setup, routing, AppState
  config.rs          .env → Config struct
  db/
    mod.rs           Connection pool init, WAL mode
    import.rs        ManaBox + CardSphere CSV import
    bulk.rs          Scryfall bulk data import
  models/mod.rs      sqlx row structs
  routes/            One file per feature area
templates/           minijinja HTML templates
static/              Served at /static (CSS, JS)
scans/               Card scan images (configurable)
scanner/             Python card recognition scripts
  capture.py         Live camera mode (Elmo OX-1)
  recognize.py       Watch-folder mode + single-image test
  hash_cards.py      Pre-compute dHashes from Scryfall
```

### Moving the database to another machine

The entire state lives in three places:

| What | Where |
|---|---|
| Database | `card-vault.db` (path from `DATABASE_URL`) |
| Card scans | `./scans/` directory (path from `SCAN_STORAGE_PATH`) |
| Bulk data cache | `./data/` directory (re-downloadable, not critical) |

Copy `card-vault.db` and `scans/` to the new machine, update `.env` paths, run `cargo run`. The bulk data cache can be re-imported from Scryfall if needed.
