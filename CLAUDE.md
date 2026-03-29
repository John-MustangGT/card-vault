# card-vault

MTG collection inventory and asset tracking system.

## Stack

- **Rust** + **axum 0.7** — web server
- **SQLite** + **sqlx 0.7** — database, compile-time checked queries
- **minijinja** — server-side templating
- **Vanilla JS** — no framework

## Running

```bash
cp .env.example .env
cargo run
# http://127.0.0.1:3000
```

Requires `sqlx-cli` for migrations during development:
```bash
cargo install sqlx-cli
sqlx db create
```

## Project Structure

```
migrations/
  0001_initial.sql        # Full schema — source of truth
src/
  main.rs                 # Server setup, routing, AppState
  config.rs               # .env → Config struct
  db/
    mod.rs                # Connection pool init, WAL mode
    import.rs             # ManaBox + CardSphere CSV import
  models/
    mod.rs                # sqlx row structs
  routes/
    import.rs             # /import GET + POST
templates/
  import.html             # Import UI (establishes visual style)
static/                   # Served at /static
scans/                    # Card scan images (configurable via .env)
```

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | `sqlite:./card-vault.db` | SQLite file path |
| `SCAN_STORAGE_PATH` | `./scans` | Front/back card scan images |
| `HOST` | `127.0.0.1` | Bind address |
| `PORT` | `3000` | Listen port |

## Schema Summary

See `migrations/0001_initial.sql` for full DDL. Key tables:

- **`scryfall_cards`** — cached Scryfall data, keyed on `scryfall_id` (UUID). Each language/printing is a unique ID.
- **`inventory_lots`** — bulk quantity tracking. `UNIQUE(scryfall_id, foil, condition)`. Re-imports accumulate quantities.
- **`individual_cards`** — high-value singles. PK is a 6-char base62 ID (e.g. `aB3xK9`) printed as QR code on Avery 5267 labels. Statuses: `in_stock | listed | out_for_grading | graded | sold`
- **`storage_locations`** — physical binders/boxes. Type: `binder | box`
- **`price_history`** — time-series price snapshots from Scryfall/TCGPlayer
- **`listings`** — marketplace listings (eBay, TCGPlayer, CardSphere, direct)
- **`grading_submissions`** — PSA/BGS/CGC grading workflow
- **`transactions`** + **`transaction_items`** — sales records for invoices/shipping

## Key Design Decisions

- **`scryfall_id` is the card identity key** — unique per language, printing, and set. Do not use name+set as a key.
- **Cost basis is nullable** — most cards have unknown acquisition cost. Never assume 0.
- **Individual card IDs** are 6-char base62, printed as QR codes. Scan output is pasted into a field; server extracts base62 via regex `[0-9A-Za-z]{6}`.
- **Scan images** go to filesystem at `{SCAN_STORAGE_PATH}/{card_id}/front.jpg` and `back.jpg`. Paths stored in DB as relative strings. Served via `tower-http` `ServeDir` at `/scans`.
- **CardSphere imports** skip rows with no Scryfall ID (sealed product) and log them as skipped — this is correct behavior.
- **ManaBox `purchase_price = 0.0`** is treated as unknown, stored as NULL.

## CSV Import Formats

Two formats are auto-detected by sniffing the header row:

**ManaBox** (detected by absence of `Tradelist Count`):
`Name, Set code, Set name, Collector number, Foil, Rarity, Quantity, ManaBox ID, Scryfall ID, Purchase price, Misprint, Altered, Condition, Language, Purchase price currency`
- Foil values: `normal | foil | etched`
- Condition values: `near_mint | lightly_played | moderately_played | heavily_played | damaged`

**CardSphere** (detected by `Tradelist Count` or `Cardsphere ID` in header):
`Count, Tradelist Count, Name, Edition, Condition, Language, Foil, Tags, Scryfall ID, Cardsphere ID, Last Modified`
- Foil values: `N | Y` → normalized to `normal | foil`
- Condition values: `NM | LP | MP | HP | DMG` → normalized to snake_case
- CardSphere provides edition name only, no set code — `set_code` and `collector_number` stored as empty strings pending Scryfall API hydration

## UI Style

Dark utilitarian aesthetic. See `templates/import.html` as the canonical reference.

- **Background:** `#0d0f11`
- **Surface:** `#151820`
- **Border:** `#252a35`
- **Accent:** `#4a9eff`
- **Text:** `#c9d1d9`
- **Fonts:** IBM Plex Mono (headings, labels, code), IBM Plex Sans (body)
- Nav height: 52px, bottom-border active indicator in accent color
- Buttons: accent background, black text, mono font

## Pending Work (Priority Order)

1. **`/inventory`** — browse/search `inventory_lots` joined to `scryfall_cards`. Filterable by set, condition, foil. Show name, set, condition, quantity, location, Scryfall price.
2. **`/individuals`** — promote a lot to a tracked individual card, assign base62 ID, upload front/back scans, set location slot.
3. **Scryfall hydration** — background `tokio` task: for cards missing `set_code`, call Scryfall API by `scryfall_id` to fill `set_code`, `collector_number`, `image_uri`, and seed `price_history`.
4. **`/locations`** — CRUD for `storage_locations` (binder/box selector).
5. **QR label generation** — generate printable Avery 5267 label sheets (1/2" × 1-3/4") with base62 ID as QR code + human-readable text.
6. **`/listings`** — create/manage marketplace listings tied to lots or individuals.
7. **Invoices + shipping labels** — transaction records, PDF invoice generation, EasyPost/Shippo API for label generation.
