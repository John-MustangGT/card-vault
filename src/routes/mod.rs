pub mod import;
pub mod inventory;
pub mod individuals;
pub mod locations;

use axum::{Router, routing::{get, post}};
use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(|| async { axum::response::Redirect::permanent("/inventory") }))
        .route("/import", get(import::get_import).post(import::post_import))
        .route("/inventory", get(inventory::get_inventory))
        .route("/individuals", get(individuals::get_individuals).post(individuals::post_individual))
        .route("/individuals/new", get(individuals::get_new_individual))
        .route("/individuals/:id", get(individuals::get_individual_detail))
        .route("/individuals/:id/status", post(individuals::post_individual_status))
        .route("/individuals/:id/scans", post(individuals::post_individual_scans))
        .route("/individuals/:id/location", post(individuals::post_individual_location))
        .route("/individuals/:id/qr", get(individuals::get_individual_qr))
        .route("/locations", get(locations::get_locations).post(locations::post_location))
        .route("/locations/new", get(locations::get_new_location))
        .route("/locations/:id", get(locations::get_location_detail).post(locations::post_location_update))
        .route("/locations/:id/delete", post(locations::post_location_delete))
        .with_state(state)
}
