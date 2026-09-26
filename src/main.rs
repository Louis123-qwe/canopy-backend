mod auth;
mod db;
use axum::{
    extract::{Json as ExtractJson, Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use sqlx::types::Json as SqlxJson;
use tower_http::cors::{CorsLayer, Any};
pub type Db = PgPool;

#[derive(Debug, Serialize, Deserialize)]
struct MilestoneInput {
    title: String,
    amount: f64,
}

#[derive(Debug, Serialize, Deserialize)]
struct CreateEscrowRequest {
    client_id: String,
    freelancer_id: String,
    amount: f64,
    description: String,
    milestones: Option<Vec<MilestoneInput>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DeliverMilestoneRequest {
    escrow_id: String,
    milestone_id: String,
    proof_note: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct DisputeEscrowRequest {
    escrow_id: String,
    reason: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct VerifyPaymentRequest {
    escrow_id: String,
    paystack_reference: String,
}

#[derive(Debug, Deserialize)]
struct PaystackVerifyResponse {
    status: bool,
    message: String,
    data: Option<PaystackVerifyData>,
}

#[derive(Debug, Deserialize)]
struct PaystackVerifyData {
    status: String,
    amount: i64,
    reference: String,
}

// ---------- Made pub, plus pub fields, so auth.rs / db.rs can
// share these same shapes for the /my-escrows endpoint. Nothing
// about their behavior changed — only visibility. ----------

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Milestone {
    pub id: String,
    pub title: String,
    pub amount: f64,
    pub status: String,
    pub proof_note: Option<String>,
    pub delivered_at: Option<String>,
    pub confirmed_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Escrow {
    pub id: String,
    pub client_id: String,
    pub freelancer_id: String,
    pub amount: f64,
    pub description: String,
    pub status: String,
    pub milestones: Vec<Milestone>,
}

#[derive(sqlx::FromRow)]
pub struct EscrowRow {
    pub id: Uuid,
    pub client_id: String,
    pub freelancer_id: String,
    pub amount: f64,
    pub description: String,
    pub status: String,
    pub milestones: SqlxJson<Vec<Milestone>>,
}

impl From<EscrowRow> for Escrow {
    fn from(row: EscrowRow) -> Self {
        Escrow {
            id: row.id.to_string(),
            client_id: row.client_id,
            freelancer_id: row.freelancer_id,
            amount: row.amount,
            description: row.description,
            status: row.status,
            milestones: row.milestones.0,
        }
    }
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let db = init_db().await;

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/", get(health_check))
        .route("/create-escrow", post(create_escrow))
        .route("/deliver-milestone", post(deliver_milestone))
        .route("/confirm-milestone", post(confirm_milestone))
        .route("/dispute", post(dispute_escrow))
        .route("/verify-payment", post(verify_payment))
        .route("/escrow/:id", get(get_escrow))
        .route("/auth/signup", post(auth::signup))
        .route("/auth/login", post(auth::login))
        .route("/my-escrows", get(auth::my_escrows)) // NEW
        .with_state(db)
        .layer(cors);

    let port = std::env::var("PORT").unwrap_or_else(|_| "3001".to_string());
let addr = format!("0.0.0.0:{}", port);
let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
println!("PayGuard escrow service running on {}", addr);

    println!("PayGuard escrow service running on port 3001");
    axum::serve(listener, app).await.unwrap();
}

async fn init_db() -> PgPool {
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL environment variable not set");

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("Failed to connect to Neon Postgres");

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS escrows (
            id UUID PRIMARY KEY,
            client_id TEXT NOT NULL,
            freelancer_id TEXT NOT NULL,
            amount DOUBLE PRECISION NOT NULL,
            description TEXT NOT NULL,
            status TEXT NOT NULL,
            milestones JSONB NOT NULL
        )
        "#,
    )
    .execute(&pool)
    .await
    .expect("Failed to create escrows table");

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS disputes (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            escrow_id TEXT NOT NULL,
            reason TEXT NOT NULL,
            raised_at TEXT NOT NULL
        )
        "#,
    )
    .execute(&pool)
    .await
    .expect("Failed to create disputes table");

    pool
}

async fn health_check() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "pay_guard" }))
}

async fn create_escrow(
    State(db): State<PgPool>,
    auth_user: auth::AuthUser,
    ExtractJson(payload): ExtractJson<CreateEscrowRequest>,
) -> Json<Value> {
    if payload.client_id != auth_user.user_id.to_string() {
        return Json(json!({
            "status": "error",
            "message": "You can only create escrows where you are the client"
        }));
    }
    let milestones: Vec<Milestone> = match payload.milestones {
        Some(inputs) => inputs
            .into_iter()
            .map(|m| Milestone {
                id: Uuid::new_v4().to_string(),
                title: m.title,
                amount: m.amount,
                status: "pending".to_string(),
                proof_note: None,
                delivered_at: None,
                confirmed_at: None,
            })
            .collect(),
        None => vec![Milestone {
            id: Uuid::new_v4().to_string(),
            title: "Full delivery".to_string(),
            amount: payload.amount,
            status: "pending".to_string(),
            proof_note: None,
            delivered_at: None,
            confirmed_at: None,
        }],
    };

    let escrow_id = Uuid::new_v4();
    let milestones_json = SqlxJson(milestones.clone());

    let result = sqlx::query(
        r#"
        INSERT INTO escrows (id, client_id, freelancer_id, amount, description, status, milestones)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(escrow_id)
    .bind(&payload.client_id)
    .bind(&payload.freelancer_id)
    .bind(payload.amount)
    .bind(&payload.description)
    .bind("pending")
    .bind(&milestones_json)
    .execute(&db)
    .await;

    match result {
        Ok(_) => {
            let escrow = Escrow {
                id: escrow_id.to_string(),
                client_id: payload.client_id,
                freelancer_id: payload.freelancer_id,
                amount: payload.amount,
                description: payload.description,
                status: "pending".to_string(),
                milestones,
            };
            Json(json!({ "status": "created", "escrow": escrow }))
        }
        Err(e) => Json(json!({ "status": "error", "message": e.to_string() })),
    }
}

async fn fetch_escrow_row(db: &PgPool, escrow_id: &str) -> Result<Option<EscrowRow>, sqlx::Error> {
    let uuid = match Uuid::parse_str(escrow_id) {
        Ok(u) => u,
        Err(_) => return Ok(None),
    };

    sqlx::query_as::<_, EscrowRow>(
        r#"
        SELECT id, client_id, freelancer_id, amount, description, status, milestones
        FROM escrows WHERE id = $1
        "#,
    )
    .bind(uuid)
    .fetch_optional(db)
    .await
}

async fn deliver_milestone(
    State(db): State<PgPool>,
    ExtractJson(payload): ExtractJson<DeliverMilestoneRequest>,
) -> Json<Value> {
    let row = match fetch_escrow_row(&db, &payload.escrow_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Json(json!({ "status": "error", "message": "escrow not found" })),
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    };

    let mut escrow: Escrow = row.into();

    let milestone_found = escrow.milestones.iter_mut().find(|m| m.id == payload.milestone_id);

    match milestone_found {
        Some(milestone) => {
            if milestone.status != "pending" {
                return Json(json!({
                    "status": "error",
                    "message": format!("milestone already in status '{}'", milestone.status)
                }));
            }
            milestone.status = "delivered".to_string();
            milestone.proof_note = Some(payload.proof_note.clone());
            milestone.delivered_at = Some(chrono::Utc::now().to_rfc3339());
        }
        None => return Json(json!({ "status": "error", "message": "milestone not found" })),
    }

    let escrow_uuid = Uuid::parse_str(&escrow.id).unwrap();
    let milestones_json = SqlxJson(escrow.milestones.clone());

    let result = sqlx::query(r#"UPDATE escrows SET milestones = $1 WHERE id = $2"#)
        .bind(&milestones_json)
        .bind(escrow_uuid)
        .execute(&db)
        .await;

    match result {
        Ok(_) => Json(json!({ "status": "delivered", "escrow": escrow })),
        Err(e) => Json(json!({ "status": "error", "message": e.to_string() })),
    }
}

async fn confirm_milestone(
    State(db): State<PgPool>,
    ExtractJson(payload): ExtractJson<DeliverMilestoneRequest>,
) -> Json<Value> {
    let row = match fetch_escrow_row(&db, &payload.escrow_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Json(json!({ "status": "error", "message": "escrow not found" })),
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    };

    let mut escrow: Escrow = row.into();

    let milestone_found = escrow.milestones.iter_mut().find(|m| m.id == payload.milestone_id);

    match milestone_found {
        Some(milestone) => {
            if milestone.status != "delivered" {
                return Json(json!({
                    "status": "error",
                    "message": format!("cannot confirm milestone in status '{}'", milestone.status)
                }));
            }
            milestone.status = "confirmed".to_string();
            milestone.confirmed_at = Some(chrono::Utc::now().to_rfc3339());
        }
        None => return Json(json!({ "status": "error", "message": "milestone not found" })),
    }

    let all_confirmed = escrow.milestones.iter().all(|m| m.status == "confirmed");
    if all_confirmed {
        escrow.status = "confirmed".to_string();
    }

    let escrow_uuid = Uuid::parse_str(&escrow.id).unwrap();
    let milestones_json = SqlxJson(escrow.milestones.clone());

    let result = sqlx::query(r#"UPDATE escrows SET milestones = $1, status = $2 WHERE id = $3"#)
        .bind(&milestones_json)
        .bind(&escrow.status)
        .bind(escrow_uuid)
        .execute(&db)
        .await;

    match result {
        Ok(_) => Json(json!({ "status": "confirmed", "escrow": escrow })),
        Err(e) => Json(json!({ "status": "error", "message": e.to_string() })),
    }
}

async fn dispute_escrow(
    State(db): State<PgPool>,
    ExtractJson(payload): ExtractJson<DisputeEscrowRequest>,
) -> Json<Value> {
    let row = match fetch_escrow_row(&db, &payload.escrow_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Json(json!({ "status": "error", "message": "escrow not found" })),
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    };

    let mut escrow: Escrow = row.into();
    escrow.status = "disputed".to_string();

    let raised_at = chrono::Utc::now().to_rfc3339();

    let log_result = sqlx::query(r#"INSERT INTO disputes (escrow_id, reason, raised_at) VALUES ($1, $2, $3)"#)
        .bind(&escrow.id)
        .bind(&payload.reason)
        .bind(&raised_at)
        .execute(&db)
        .await;

    if let Err(e) = log_result {
        return Json(json!({ "status": "error", "message": e.to_string() }));
    }

    let escrow_uuid = Uuid::parse_str(&escrow.id).unwrap();

    let update_result = sqlx::query(r#"UPDATE escrows SET status = $1 WHERE id = $2"#)
        .bind(&escrow.status)
        .bind(escrow_uuid)
        .execute(&db)
        .await;

    match update_result {
        Ok(_) => Json(json!({ "status": "disputed", "escrow": escrow })),
        Err(e) => Json(json!({ "status": "error", "message": e.to_string() })),
    }
}

async fn verify_payment(
    State(db): State<PgPool>,
    ExtractJson(payload): ExtractJson<VerifyPaymentRequest>,
) -> Json<Value> {
    let row = match fetch_escrow_row(&db, &payload.escrow_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Json(json!({ "status": "error", "message": "escrow not found" })),
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    };

    let mut escrow: Escrow = row.into();

    let paystack_secret = match std::env::var("PAYSTACK_SECRET_KEY") {
        Ok(key) => key,
        Err(_) => {
            return Json(json!({
                "status": "error",
                "message": "PAYSTACK_SECRET_KEY not set in environment"
            }));
        }
    };

    let url = format!("https://api.paystack.co/transaction/verify/{}", payload.paystack_reference);

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", paystack_secret))
        .send()
        .await;

    let verify_data: PaystackVerifyResponse = match response {
        Ok(resp) => match resp.json().await {
            Ok(data) => data,
            Err(e) => {
                return Json(json!({
                    "status": "error",
                    "message": format!("failed to parse Paystack response: {}", e)
                }));
            }
        },
        Err(e) => {
            return Json(json!({
                "status": "error",
                "message": format!("failed to reach Paystack: {}", e)
            }));
        }
    };

    if !verify_data.status {
        return Json(json!({
            "status": "error",
            "message": format!("Paystack verification failed: {}", verify_data.message)
        }));
    }

    let data = match verify_data.data {
        Some(d) => d,
        None => return Json(json!({ "status": "error", "message": "no transaction data returned" })),
    };

    if data.status != "success" {
        return Json(json!({
            "status": "error",
            "message": format!("transaction status is '{}', not success", data.status)
        }));
    }

    let paid_naira = data.amount as f64 / 100.0;
    if (paid_naira - escrow.amount).abs() > 0.01 {
        return Json(json!({
            "status": "error",
            "message": format!("amount mismatch: expected {}, Paystack confirms {}", escrow.amount, paid_naira)
        }));
    }

    escrow.status = "funded".to_string();

    let escrow_uuid = Uuid::parse_str(&escrow.id).unwrap();

    let result = sqlx::query(r#"UPDATE escrows SET status = $1 WHERE id = $2"#)
        .bind(&escrow.status)
        .bind(escrow_uuid)
        .execute(&db)
        .await;

    match result {
        Ok(_) => Json(json!({ "status": "funded", "escrow": escrow })),
        Err(e) => Json(json!({ "status": "error", "message": e.to_string() })),
    }
}

async fn get_escrow(State(db): State<PgPool>, Path(id): Path<String>) -> Json<Value> {
    match fetch_escrow_row(&db, &id).await {
        Ok(Some(row)) => {
            let escrow: Escrow = row.into();
            Json(json!({ "status": "ok", "escrow": escrow }))
        }
        Ok(None) => Json(json!({ "status": "error", "message": "escrow not found" })),
        Err(e) => Json(json!({ "status": "error", "message": e.to_string() })),
    }
}
