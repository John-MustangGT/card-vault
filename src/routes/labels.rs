use axum::{
    extract::{Form, Query, State},
    response::{Html, Redirect},
    Json,
};
use serde::Deserialize;
use sqlx::Row;
use std::sync::Arc;

use crate::AppState;

fn gen_uid() -> String {
    let id = uuid::Uuid::new_v4();
    let bytes = id.as_bytes();
    let alphabet = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    (0..6).map(|i| alphabet[bytes[i] as usize % 62] as char).collect()
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

pub async fn labels_page(State(state): State<Arc<AppState>>) -> Html<String> {
    let total: i64 = sqlx::query("SELECT COUNT(*) as cnt FROM uid_pool")
        .fetch_one(&state.pool)
        .await
        .map(|r| r.get::<i64, _>("cnt"))
        .unwrap_or(0);

    let unused: i64 = sqlx::query("SELECT COUNT(*) as cnt FROM uid_pool WHERE used = 0")
        .fetch_one(&state.pool)
        .await
        .map(|r| r.get::<i64, _>("cnt"))
        .unwrap_or(0);

    let pool_rows = sqlx::query(
        "SELECT uid FROM uid_pool WHERE used = 0 ORDER BY id LIMIT 100",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let pool_uids: Vec<String> = pool_rows
        .iter()
        .map(|r| r.get::<String, _>("uid"))
        .collect();

    let tmpl = state.env.get_template("labels.html").expect("labels.html missing");
    let ctx = minijinja::context! {
        total   => total,
        unused  => unused,
        used    => total - unused,
        pool_uids => pool_uids,
    };
    Html(tmpl.render(ctx).expect("template render failed"))
}

#[derive(Deserialize)]
pub struct GenerateForm {
    pub count: Option<i64>,
}

pub async fn generate_uids(
    State(state): State<Arc<AppState>>,
    Form(form): Form<GenerateForm>,
) -> Redirect {
    let count = form.count.unwrap_or(80).clamp(1, 800);
    let now = unix_now();
    for _ in 0..count {
        // Retry a few times on collision (astronomically rare but correct)
        for _ in 0..10 {
            let uid = gen_uid();
            let ok = sqlx::query(
                "INSERT OR IGNORE INTO uid_pool (uid, used, created_at) VALUES (?, 0, ?)",
            )
            .bind(&uid)
            .bind(now)
            .execute(&state.pool)
            .await
            .map(|r| r.rows_affected() > 0)
            .unwrap_or(false);
            if ok {
                break;
            }
        }
    }
    Redirect::to("/labels")
}

#[derive(Deserialize)]
pub struct PrintQuery {
    pub brand: Option<String>,
    pub count: Option<i64>,
}

pub async fn labels_print(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PrintQuery>,
) -> Html<String> {
    let count = params.count.unwrap_or(80).clamp(1, 80);
    let brand = params
        .brand
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Direwolf Card Vault".to_string());

    let rows = sqlx::query("SELECT uid FROM uid_pool WHERE used = 0 ORDER BY id LIMIT ?")
        .bind(count)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

    let uids: Vec<String> = rows.iter().map(|r| r.get::<String, _>("uid")).collect();

    let tmpl = state
        .env
        .get_template("labels_print.html")
        .expect("labels_print.html missing");
    let ctx = minijinja::context! {
        uids  => uids,
        brand => brand,
    };
    Html(tmpl.render(ctx).expect("template render failed"))
}

pub async fn next_uid(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let row =
        sqlx::query("SELECT uid FROM uid_pool WHERE used = 0 ORDER BY id LIMIT 1")
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None);

    match row {
        Some(r) => Json(serde_json::json!({ "ok": true, "uid": r.get::<String, _>("uid") })),
        None => Json(serde_json::json!({ "ok": false, "uid": null })),
    }
}
