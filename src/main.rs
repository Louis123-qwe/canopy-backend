mod auth;
mod db;
mod email;
mod google;
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
use chrono::NaiveDate;
pub type Db = PgPool;

#[derive(Debug, Serialize, Deserialize)]
struct MilestoneInput {
    title: String,
    amount: i64,
    deadline: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ProposeEscrowRequest {
    client_id: String,
    amount: i64,
    description: String,
    milestones: Option<Vec<MilestoneInput>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RespondProposalRequest {
    escrow_id: String,
    accept: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct UpdateProposalRequest {
    escrow_id: String,
    amount: i64,
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
    proposed_split_freelancer_pct: f64,
}

#[derive(Debug, Serialize, Deserialize)]
struct RespondDisputeRequest {
    escrow_id: String,
    action: String,
    counter_split_freelancer_pct: Option<f64>,
    counter_reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RespondCounterRequest {
    escrow_id: String,
    accept: bool,
}

#[derive(Debug, Deserialize)]
struct RequestExtensionRequest {
    escrow_id: String,
    milestone_id: String,
    new_deadline: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct RespondExtensionRequest {
    request_id: String,
    accept: bool,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct ExtensionRow {
    id: Uuid,
    escrow_id: Uuid,
    milestone_id: String,
    requested_by: String,
    current_deadline: Option<String>,
    new_deadline: String,
    reason: String,
    status: String,
}

#[derive(Debug, Deserialize)]
struct AdminResolveRequest {
    dispute_id: String,
    freelancer_pct: f64,
    note: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct DisputeRow {
    id: Uuid,
    escrow_id: Uuid,
    raised_by: String,
    reason: String,
    proposed_split_freelancer_pct: f64,
    status: String,
    counter_split_freelancer_pct: Option<f64>,
    counter_reason: Option<String>,
    last_action_by: String,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct AdminDisputeRow {
    id: Uuid,
    escrow_id: Uuid,
    raised_by: String,
    reason: String,
    proposed_split_freelancer_pct: f64,
    counter_split_freelancer_pct: Option<f64>,
    counter_reason: Option<String>,
    last_action_by: String,
    created_at: String,
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Milestone {
    pub id: String,
    pub title: String,
    pub amount: i64,
    pub status: String,
    pub deadline: Option<String>,
    pub proof_note: Option<String>,
    pub delivered_at: Option<String>,
    pub confirmed_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Escrow {
    pub id: String,
    pub client_id: String,
    pub freelancer_id: String,
    pub amount: i64,
    pub description: String,
    pub status: String,
    pub milestones: Vec<Milestone>,
}

#[derive(sqlx::FromRow)]
pub struct EscrowRow {
    pub id: Uuid,
    pub client_id: String,
    pub freelancer_id: String,
    pub amount: i64,
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

    std::env::var("JWT_SECRET").expect("JWT_SECRET environment variable not set");

    let db = init_db().await;

    let sweep_db = db.clone();
    tokio::spawn(async move {
        run_dispute_sweep(sweep_db).await;
    });

    let cors = CorsLayer::new()
        .allow_origin("https://canopy-lime.vercel.app".parse::<axum::http::HeaderValue>().unwrap())
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/", get(health_check))
        .route("/propose-escrow", post(propose_escrow))
        .route("/respond-proposal", post(respond_to_proposal))
        .route("/update-proposal", post(update_proposal))
        .route("/deliver-milestone", post(deliver_milestone))
        .route("/confirm-milestone", post(confirm_milestone))
        .route("/dispute", post(dispute_escrow))
        .route("/respond-dispute", post(respond_dispute))
        .route("/respond-counter", post(respond_counter))
        .route("/request-extension", post(request_extension))
        .route("/respond-extension", post(respond_extension))
        .route("/verify-payment", post(verify_payment))
        .route("/escrow/:id", get(get_escrow))
        .route("/users/name/:id", get(user_name))
        .route("/admin/check", get(admin_check))
        .route("/admin/disputes", get(admin_list_disputes))
        .route("/admin/disputes/resolve", post(admin_resolve_dispute))
        .route("/auth/signup", post(auth::signup))
        .route("/auth/login", post(auth::login))
        .route("/auth/google", post(google::google_login))
        .route("/my-escrows", get(auth::my_escrows))
        .route("/users/lookup", get(auth::lookup_user))
        .with_state(db)
        .layer(cors);

    let port = std::env::var("PORT").unwrap_or_else(|_| "3001".to_string());
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    println!("Canopy escrow service running on {}", addr);

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
            amount BIGINT NOT NULL,
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
            escrow_id UUID NOT NULL,
            raised_by TEXT NOT NULL,
            reason TEXT NOT NULL,
            proposed_split_freelancer_pct DOUBLE PRECISION NOT NULL,
            status TEXT NOT NULL DEFAULT 'open',
            counter_split_freelancer_pct DOUBLE PRECISION,
            counter_reason TEXT,
            last_action_at TIMESTAMPTZ NOT NULL DEFAULT now(),
            last_action_by TEXT NOT NULL,
            resolved_at TIMESTAMPTZ,
            created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
            final_split_freelancer_pct DOUBLE PRECISION,
            resolved_by TEXT,
            admin_note TEXT
        )
        "#,
    )
    .execute(&pool)
    .await
    .expect("Failed to create disputes table");

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS extension_requests (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            escrow_id UUID NOT NULL,
            milestone_id TEXT NOT NULL,
            requested_by TEXT NOT NULL,
            current_deadline TEXT,
            new_deadline TEXT NOT NULL,
            reason TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
            responded_at TIMESTAMPTZ
        )
        "#,
    )
    .execute(&pool)
    .await
    .expect("Failed to create extension_requests table");

    pool
}

#[derive(sqlx::FromRow)]
struct SweepDisputeRow {
    id: Uuid,
    escrow_id: Uuid,
    status: String,
    last_action_by: String,
}

async fn run_dispute_sweep(db: PgPool) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;

        let reminders = sqlx::query_as::<_, SweepDisputeRow>(
            r#"
            SELECT id, escrow_id, status, last_action_by FROM disputes
            WHERE status IN ('open', 'countered')
            AND last_action_at < now() - INTERVAL '72 hours'
            AND last_action_at >= now() - INTERVAL '73 hours'
            "#,
        )
        .fetch_all(&db)
        .await
        .unwrap_or_default();

        for d in reminders {
            if let Ok(Some(row)) = fetch_escrow_row(&db, &d.escrow_id.to_string()).await {
                let escrow: Escrow = row.into();
                let waiting_on_id = if d.last_action_by == escrow.client_id {
                    &escrow.freelancer_id
                } else {
                    &escrow.client_id
                };
                if let Ok(Some(user)) = db::find_user_by_id(&db, waiting_on_id).await {
                    email::dispute_reminder(&user.email, &escrow.id).await;
                }
            }
        }

        let overdue = sqlx::query_as::<_, SweepDisputeRow>(
            r#"
            SELECT id, escrow_id, status, last_action_by FROM disputes
            WHERE status IN ('open', 'countered')
            AND last_action_at < now() - INTERVAL '96 hours'
            "#,
        )
        .fetch_all(&db)
        .await
        .unwrap_or_default();

        for d in overdue {
            let update = sqlx::query(
                r#"UPDATE disputes SET status = 'escalated', last_action_at = now() WHERE id = $1"#,
            )
            .bind(d.id)
            .execute(&db)
            .await;

            if update.is_ok() {
                if let Ok(Some(row)) = fetch_escrow_row(&db, &d.escrow_id.to_string()).await {
                    let escrow: Escrow = row.into();
                    if let Ok(Some(client)) = db::find_user_by_id(&db, &escrow.client_id).await {
                        email::dispute_escalated(&client.email, &escrow.id).await;
                    }
                    if let Ok(Some(freelancer)) = db::find_user_by_id(&db, &escrow.freelancer_id).await {
                        email::dispute_escalated(&freelancer.email, &escrow.id).await;
                    }
                }
            }
        }
    }
}

async fn health_check() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "canopy" }))
}

async fn propose_escrow(
    State(db): State<PgPool>,
    auth_user: auth::AuthUser,
    ExtractJson(payload): ExtractJson<ProposeEscrowRequest>,
) -> Json<Value> {
    if let Err(msg) = validate_proposal(payload.amount, &payload.description, &payload.milestones) {
        return Json(json!({ "status": "error", "message": msg }));
    }

    if payload.client_id == auth_user.user_id.to_string() {
        return Json(json!({ "status": "error", "message": "You cannot propose an escrow to yourself" }));
    }

    match db::find_user_by_id(&db, &payload.client_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Json(json!({ "status": "error", "message": "client not found" })),
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    }

    let milestones: Vec<Milestone> = build_milestones(payload.milestones, payload.amount);

    let escrow_id = Uuid::new_v4();
    let milestones_json = SqlxJson(milestones.clone());
    let freelancer_id = auth_user.user_id.to_string();

    let result = sqlx::query(
        r#"
        INSERT INTO escrows (id, client_id, freelancer_id, amount, description, status, milestones)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(escrow_id)
    .bind(&payload.client_id)
    .bind(&freelancer_id)
    .bind(payload.amount)
    .bind(&payload.description)
    .bind("proposed")
    .bind(&milestones_json)
    .execute(&db)
    .await;

    match result {
        Ok(_) => {
            let escrow = Escrow {
                id: escrow_id.to_string(),
                client_id: payload.client_id,
                freelancer_id: freelancer_id.clone(),
                amount: payload.amount,
                description: payload.description,
                status: "proposed".to_string(),
                milestones,
            };

            if let Ok(Some(client)) = db::find_user_by_id(&db, &escrow.client_id).await {
                if let Ok(Some(freelancer)) = db::find_user_by_id(&db, &freelancer_id).await {
                    let freelancer_name = format!("{} {}", freelancer.first_name, freelancer.last_name);
                    email::proposal_sent(&client.email, &freelancer_name, &escrow.id).await;
                }
            }

            Json(json!({ "status": "proposed", "escrow": escrow }))
        }
        Err(e) => Json(json!({ "status": "error", "message": e.to_string() })),
    }
}

const MAX_AMOUNT_KOBO: i64 = 100_000_000_000;

fn validate_amounts(amount: i64, milestones: &Option<Vec<MilestoneInput>>) -> Result<(), String> {
    if amount <= 0 {
        return Err("amount must be greater than zero".to_string());
    }
    if amount > MAX_AMOUNT_KOBO {
        return Err("amount is too large (the limit is \u{20A6}1,000,000,000)".to_string());
    }
    if let Some(list) = milestones {
        if list.is_empty() {
            return Err("add at least one milestone".to_string());
        }
        let mut total: i64 = 0;
        for m in list {
            if m.amount <= 0 {
                return Err("every milestone needs an amount greater than zero".to_string());
            }
            total = match total.checked_add(m.amount) {
                Some(t) => t,
                None => return Err("amount is too large".to_string()),
            };
        }
        if total != amount {
            return Err("milestone amounts must add up to the total".to_string());
        }
    }
    Ok(())
}

fn validate_proposal(
    amount: i64,
    description: &str,
    milestones: &Option<Vec<MilestoneInput>>,
) -> Result<(), String> {
    let desc = description.trim();
    if desc.is_empty() {
        return Err("add a short description of the work".to_string());
    }
    if desc.chars().count() > 500 {
        return Err("the description can be 500 characters at most".to_string());
    }

    validate_amounts(amount, milestones)?;

    if let Some(list) = milestones {
        for m in list {
            let title = m.title.trim();
            if title.is_empty() {
                return Err("every milestone needs a title".to_string());
            }
            if title.chars().count() > 120 {
                return Err("milestone titles can be 120 characters at most".to_string());
            }
            if let Some(d) = &m.deadline {
                if !d.is_empty() && NaiveDate::parse_from_str(d, "%Y-%m-%d").is_err() {
                    return Err("milestone deadlines must be valid dates".to_string());
                }
            }
        }
    }

    Ok(())
}

fn funding_required() -> bool {
    std::env::var("REQUIRE_FUNDING").map(|v| v == "true").unwrap_or(false)
}

fn escrow_is_active(status: &str) -> bool {
    if funding_required() {
        status == "funded"
    } else {
        status == "funded" || status == "accepted"
    }
}

fn build_milestones(inputs: Option<Vec<MilestoneInput>>, total_amount: i64) -> Vec<Milestone> {
    match inputs {
        Some(list) => list
            .into_iter()
            .map(|m| Milestone {
                id: Uuid::new_v4().to_string(),
                title: m.title,
                amount: m.amount,
                status: "pending".to_string(),
                deadline: m.deadline,
                proof_note: None,
                delivered_at: None,
                confirmed_at: None,
            })
            .collect(),
        None => vec![Milestone {
            id: Uuid::new_v4().to_string(),
            title: "Full delivery".to_string(),
            amount: total_amount,
            status: "pending".to_string(),
            deadline: None,
            proof_note: None,
            delivered_at: None,
            confirmed_at: None,
        }],
    }
}

async fn respond_to_proposal(
    State(db): State<PgPool>,
    auth_user: auth::AuthUser,
    ExtractJson(payload): ExtractJson<RespondProposalRequest>,
) -> Json<Value> {
    let row = match fetch_escrow_row(&db, &payload.escrow_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Json(json!({ "status": "error", "message": "escrow not found" })),
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    };

    let mut escrow: Escrow = row.into();

    if escrow.client_id != auth_user.user_id.to_string() {
        return Json(json!({
            "status": "error",
            "message": "Only the client on this escrow can accept or reject it"
        }));
    }

    if escrow.status != "proposed" {
        return Json(json!({
            "status": "error",
            "message": format!("cannot respond to an escrow in status '{}'", escrow.status)
        }));
    }

    escrow.status = if payload.accept { "accepted".to_string() } else { "rejected".to_string() };

    let escrow_uuid = Uuid::parse_str(&escrow.id).unwrap();
    let result = sqlx::query(r#"UPDATE escrows SET status = $1 WHERE id = $2"#)
        .bind(&escrow.status)
        .bind(escrow_uuid)
        .execute(&db)
        .await;

    match result {
        Ok(_) => {
            if let Ok(Some(freelancer)) = db::find_user_by_id(&db, &escrow.freelancer_id).await {
                if payload.accept {
                    email::proposal_accepted(&freelancer.email, &escrow.id).await;
                } else {
                    email::proposal_rejected(&freelancer.email, &escrow.id).await;
                }
            }
            Json(json!({ "status": escrow.status.clone(), "escrow": escrow }))
        }
        Err(e) => Json(json!({ "status": "error", "message": e.to_string() })),
    }
}

async fn update_proposal(
    State(db): State<PgPool>,
    auth_user: auth::AuthUser,
    ExtractJson(payload): ExtractJson<UpdateProposalRequest>,
) -> Json<Value> {
    let row = match fetch_escrow_row(&db, &payload.escrow_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Json(json!({ "status": "error", "message": "escrow not found" })),
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    };

    let mut escrow: Escrow = row.into();

    if escrow.freelancer_id != auth_user.user_id.to_string() {
        return Json(json!({
            "status": "error",
            "message": "Only the freelancer who proposed this escrow can edit it"
        }));
    }

    if escrow.status != "proposed" && escrow.status != "rejected" {
        return Json(json!({
            "status": "error",
            "message": format!("cannot edit an escrow in status '{}'", escrow.status)
        }));
    }

    if let Err(msg) = validate_proposal(payload.amount, &payload.description, &payload.milestones) {
        return Json(json!({ "status": "error", "message": msg }));
    }

    let milestones = build_milestones(payload.milestones, payload.amount);
    let milestones_json = SqlxJson(milestones.clone());

    escrow.amount = payload.amount;
    escrow.description = payload.description;
    escrow.milestones = milestones;
    escrow.status = "proposed".to_string();

    let escrow_uuid = Uuid::parse_str(&escrow.id).unwrap();
    let result = sqlx::query(
        r#"UPDATE escrows SET amount = $1, description = $2, milestones = $3, status = $4 WHERE id = $5"#,
    )
    .bind(escrow.amount)
    .bind(&escrow.description)
    .bind(&milestones_json)
    .bind(&escrow.status)
    .bind(escrow_uuid)
    .execute(&db)
    .await;

    match result {
        Ok(_) => Json(json!({ "status": "proposed", "escrow": escrow })),
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
    auth_user: auth::AuthUser,
    ExtractJson(payload): ExtractJson<DeliverMilestoneRequest>,
) -> Json<Value> {
    let row = match fetch_escrow_row(&db, &payload.escrow_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Json(json!({ "status": "error", "message": "escrow not found" })),
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    };

    let mut escrow: Escrow = row.into();

    if escrow.freelancer_id != auth_user.user_id.to_string() {
        return Json(json!({
            "status": "error",
            "message": "Only the freelancer on this escrow can deliver a milestone"
        }));
    }

    if !escrow_is_active(&escrow.status) {
        return Json(json!({
            "status": "error",
            "message": format!("cannot deliver a milestone while the escrow is '{}'", escrow.status)
        }));
    }

    if payload.proof_note.chars().count() > 1000 {
        return Json(json!({ "status": "error", "message": "the delivery note can be 1000 characters at most" }));
    }

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
        Ok(_) => {
            if let Ok(Some(client)) = db::find_user_by_id(&db, &escrow.client_id).await {
                email::milestone_delivered(&client.email, &escrow.id).await;
            }
            Json(json!({ "status": "delivered", "escrow": escrow }))
        }
        Err(e) => Json(json!({ "status": "error", "message": e.to_string() })),
    }
}

async fn confirm_milestone(
    State(db): State<PgPool>,
    auth_user: auth::AuthUser,
    ExtractJson(payload): ExtractJson<DeliverMilestoneRequest>,
) -> Json<Value> {
    let row = match fetch_escrow_row(&db, &payload.escrow_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Json(json!({ "status": "error", "message": "escrow not found" })),
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    };

    let mut escrow: Escrow = row.into();

    if escrow.client_id != auth_user.user_id.to_string() {
        return Json(json!({
            "status": "error",
            "message": "Only the client on this escrow can confirm a milestone"
        }));
    }

    if !escrow_is_active(&escrow.status) {
        return Json(json!({
            "status": "error",
            "message": format!("cannot confirm a milestone while the escrow is '{}'", escrow.status)
        }));
    }

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
        Ok(_) => {
            if let Ok(Some(freelancer)) = db::find_user_by_id(&db, &escrow.freelancer_id).await {
                email::milestone_confirmed(&freelancer.email, &escrow.id).await;
            }
            Json(json!({ "status": "confirmed", "escrow": escrow }))
        }
        Err(e) => Json(json!({ "status": "error", "message": e.to_string() })),
    }
}

async fn fetch_open_dispute(db: &PgPool, escrow_id: &str) -> Result<Option<DisputeRow>, sqlx::Error> {
    let uuid = match Uuid::parse_str(escrow_id) {
        Ok(u) => u,
        Err(_) => return Ok(None),
    };

    sqlx::query_as::<_, DisputeRow>(
        r#"
        SELECT id, escrow_id, raised_by, reason, proposed_split_freelancer_pct,
               status, counter_split_freelancer_pct, counter_reason, last_action_by
        FROM disputes
        WHERE escrow_id = $1 AND status IN ('open', 'countered')
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(uuid)
    .fetch_optional(db)
    .await
}

async fn fetch_visible_dispute(db: &PgPool, escrow_id: &str) -> Result<Option<DisputeRow>, sqlx::Error> {
    let uuid = match Uuid::parse_str(escrow_id) {
        Ok(u) => u,
        Err(_) => return Ok(None),
    };

    sqlx::query_as::<_, DisputeRow>(
        r#"
        SELECT id, escrow_id, raised_by, reason, proposed_split_freelancer_pct,
               status, counter_split_freelancer_pct, counter_reason, last_action_by
        FROM disputes
        WHERE escrow_id = $1 AND status IN ('open', 'countered', 'escalated')
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(uuid)
    .fetch_optional(db)
    .await
}

async fn apply_split_and_close(
    db: &PgPool,
    escrow: &Escrow,
    dispute_id: Uuid,
    freelancer_pct: f64,
) -> Result<(), sqlx::Error> {
    let escrow_uuid = Uuid::parse_str(&escrow.id).unwrap();

    sqlx::query(r#"UPDATE escrows SET status = 'confirmed' WHERE id = $1"#)
        .bind(escrow_uuid)
        .execute(db)
        .await?;

    sqlx::query(
        r#"UPDATE disputes SET status = 'resolved', resolved_at = now(),
           final_split_freelancer_pct = $1,
           counter_split_freelancer_pct = COALESCE(counter_split_freelancer_pct, $1)
           WHERE id = $2"#,
    )
    .bind(freelancer_pct)
    .bind(dispute_id)
    .execute(db)
    .await?;

    Ok(())
}

async fn dispute_escrow(
    State(db): State<PgPool>,
    auth_user: auth::AuthUser,
    ExtractJson(payload): ExtractJson<DisputeEscrowRequest>,
) -> Json<Value> {
    let row = match fetch_escrow_row(&db, &payload.escrow_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Json(json!({ "status": "error", "message": "escrow not found" })),
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    };

    let mut escrow: Escrow = row.into();
    let caller_id = auth_user.user_id.to_string();

    if escrow.client_id != caller_id {
        return Json(json!({
            "status": "error",
            "message": "Only the client on this escrow can raise a dispute"
        }));
    }

    if !escrow_is_active(&escrow.status) {
        return Json(json!({
            "status": "error",
            "message": format!("cannot raise a dispute while the escrow is '{}'", escrow.status)
        }));
    }

    if !(0.0..=100.0).contains(&payload.proposed_split_freelancer_pct) {
        return Json(json!({ "status": "error", "message": "split must be between 0 and 100" }));
    }

    let dispute_reason = payload.reason.trim().to_string();
    if dispute_reason.is_empty() || dispute_reason.chars().count() > 1000 {
        return Json(json!({ "status": "error", "message": "give a reason of up to 1000 characters" }));
    }

    escrow.status = "disputed".to_string();
    let escrow_uuid = Uuid::parse_str(&escrow.id).unwrap();

    let insert_result = sqlx::query(
        r#"
        INSERT INTO disputes (escrow_id, raised_by, reason, proposed_split_freelancer_pct, status, last_action_by)
        VALUES ($1, $2, $3, $4, 'open', $2)
        "#,
    )
    .bind(escrow_uuid)
    .bind(&caller_id)
    .bind(&dispute_reason)
    .bind(payload.proposed_split_freelancer_pct)
    .execute(&db)
    .await;

    if let Err(e) = insert_result {
        return Json(json!({ "status": "error", "message": e.to_string() }));
    }

    let update_result = sqlx::query(r#"UPDATE escrows SET status = $1 WHERE id = $2"#)
        .bind(&escrow.status)
        .bind(escrow_uuid)
        .execute(&db)
        .await;

    match update_result {
        Ok(_) => {
            if let Ok(Some(freelancer)) = db::find_user_by_id(&db, &escrow.freelancer_id).await {
                email::dispute_raised(&freelancer.email, &escrow.id).await;
            }
            Json(json!({ "status": "disputed", "escrow": escrow }))
        }
        Err(e) => Json(json!({ "status": "error", "message": e.to_string() })),
    }
}

async fn respond_dispute(
    State(db): State<PgPool>,
    auth_user: auth::AuthUser,
    ExtractJson(payload): ExtractJson<RespondDisputeRequest>,
) -> Json<Value> {
    let escrow_row = match fetch_escrow_row(&db, &payload.escrow_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Json(json!({ "status": "error", "message": "escrow not found" })),
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    };
    let escrow: Escrow = escrow_row.into();
    let caller_id = auth_user.user_id.to_string();

    if escrow.freelancer_id != caller_id {
        return Json(json!({ "status": "error", "message": "Only the freelancer can respond to this dispute" }));
    }

    let dispute = match fetch_open_dispute(&db, &payload.escrow_id).await {
        Ok(Some(d)) => d,
        Ok(None) => return Json(json!({ "status": "error", "message": "no open dispute on this escrow" })),
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    };

    match payload.action.as_str() {
        "accept" => {
            if let Err(e) = apply_split_and_close(&db, &escrow, dispute.id, dispute.proposed_split_freelancer_pct).await {
                return Json(json!({ "status": "error", "message": e.to_string() }));
            }
            if let Ok(Some(client)) = db::find_user_by_id(&db, &escrow.client_id).await {
                email::dispute_resolved(&client.email, &escrow.id).await;
            }
            if let Ok(Some(freelancer)) = db::find_user_by_id(&db, &escrow.freelancer_id).await {
                email::dispute_resolved(&freelancer.email, &escrow.id).await;
            }
            Json(json!({ "status": "resolved", "freelancer_pct": dispute.proposed_split_freelancer_pct }))
        }
        "reject" => {
            let reason = match &payload.counter_reason {
                Some(r) if !r.trim().is_empty() && r.chars().count() <= 1000 => r.trim().to_string(),
                _ => return Json(json!({ "status": "error", "message": "give a reason of up to 1000 characters to reject" })),
            };
            let result = sqlx::query(
                r#"UPDATE disputes SET status = 'escalated', counter_reason = $1,
                   last_action_by = $2, last_action_at = now() WHERE id = $3"#,
            )
            .bind(&reason)
            .bind(&caller_id)
            .bind(dispute.id)
            .execute(&db)
            .await;
            match result {
                Ok(_) => {
                    if let Ok(Some(client)) = db::find_user_by_id(&db, &escrow.client_id).await {
                        email::dispute_escalated(&client.email, &escrow.id).await;
                    }
                    Json(json!({ "status": "escalated" }))
                }
                Err(e) => Json(json!({ "status": "error", "message": e.to_string() })),
            }
        }
        "counter" => {
            let pct = match payload.counter_split_freelancer_pct {
                Some(p) if (0.0..=100.0).contains(&p) => p,
                _ => return Json(json!({ "status": "error", "message": "a valid counter split (0-100) is required" })),
            };
            let result = sqlx::query(
                r#"UPDATE disputes SET status = 'countered', counter_split_freelancer_pct = $1,
                   last_action_by = $2, last_action_at = now() WHERE id = $3"#,
            )
            .bind(pct)
            .bind(&caller_id)
            .bind(dispute.id)
            .execute(&db)
            .await;
            match result {
                Ok(_) => {
                    if let Ok(Some(client)) = db::find_user_by_id(&db, &escrow.client_id).await {
                        email::dispute_countered(&client.email, &escrow.id).await;
                    }
                    Json(json!({ "status": "countered", "counter_split_freelancer_pct": pct }))
                }
                Err(e) => Json(json!({ "status": "error", "message": e.to_string() })),
            }
        }
        _ => Json(json!({ "status": "error", "message": "action must be accept, reject, or counter" })),
    }
}

async fn respond_counter(
    State(db): State<PgPool>,
    auth_user: auth::AuthUser,
    ExtractJson(payload): ExtractJson<RespondCounterRequest>,
) -> Json<Value> {
    let escrow_row = match fetch_escrow_row(&db, &payload.escrow_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Json(json!({ "status": "error", "message": "escrow not found" })),
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    };
    let escrow: Escrow = escrow_row.into();
    let caller_id = auth_user.user_id.to_string();

    if escrow.client_id != caller_id {
        return Json(json!({ "status": "error", "message": "Only the client can respond to a counter-offer" }));
    }

    let dispute = match fetch_open_dispute(&db, &payload.escrow_id).await {
        Ok(Some(d)) if d.status == "countered" => d,
        Ok(_) => return Json(json!({ "status": "error", "message": "no pending counter-offer on this escrow" })),
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    };

    if payload.accept {
        let pct = dispute.counter_split_freelancer_pct.unwrap_or(dispute.proposed_split_freelancer_pct);
        if let Err(e) = apply_split_and_close(&db, &escrow, dispute.id, pct).await {
            return Json(json!({ "status": "error", "message": e.to_string() }));
        }
        if let Ok(Some(client)) = db::find_user_by_id(&db, &escrow.client_id).await {
            email::dispute_resolved(&client.email, &escrow.id).await;
        }
        if let Ok(Some(freelancer)) = db::find_user_by_id(&db, &escrow.freelancer_id).await {
            email::dispute_resolved(&freelancer.email, &escrow.id).await;
        }
        Json(json!({ "status": "resolved", "freelancer_pct": pct }))
    } else {
        let result = sqlx::query(
            r#"UPDATE disputes SET status = 'escalated', last_action_by = $1, last_action_at = now() WHERE id = $2"#,
        )
        .bind(&caller_id)
        .bind(dispute.id)
        .execute(&db)
        .await;
        match result {
            Ok(_) => {
                if let Ok(Some(freelancer)) = db::find_user_by_id(&db, &escrow.freelancer_id).await {
                    email::dispute_escalated(&freelancer.email, &escrow.id).await;
                }
                Json(json!({ "status": "escalated" }))
            }
            Err(e) => Json(json!({ "status": "error", "message": e.to_string() })),
        }
    }
}

async fn is_admin(db: &PgPool, user_id: &str) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT is_admin FROM users WHERE id::text = $1")
        .bind(user_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
}

async fn admin_check(State(db): State<PgPool>, auth_user: auth::AuthUser) -> Json<Value> {
    let admin = is_admin(&db, &auth_user.user_id.to_string()).await;
    Json(json!({ "is_admin": admin }))
}

async fn admin_list_disputes(State(db): State<PgPool>, auth_user: auth::AuthUser) -> Json<Value> {
    if !is_admin(&db, &auth_user.user_id.to_string()).await {
        return Json(json!({ "status": "error", "message": "Admin access required" }));
    }

    let rows = sqlx::query_as::<_, AdminDisputeRow>(
        r#"
        SELECT id, escrow_id, raised_by, reason, proposed_split_freelancer_pct,
               counter_split_freelancer_pct, counter_reason, last_action_by,
               created_at::text AS created_at
        FROM disputes
        WHERE status = 'escalated'
        ORDER BY last_action_at ASC
        "#,
    )
    .fetch_all(&db)
    .await;

    match rows {
        Ok(list) => {
            let mut out = Vec::new();
            for d in list {
                let escrow = fetch_escrow_row(&db, &d.escrow_id.to_string())
                    .await
                    .ok()
                    .flatten()
                    .map(Escrow::from);
                let client_name = match &escrow {
                    Some(e) => user_display_name(&db, &e.client_id).await,
                    None => String::new(),
                };
                let freelancer_name = match &escrow {
                    Some(e) => user_display_name(&db, &e.freelancer_id).await,
                    None => String::new(),
                };
                out.push(json!({
                    "dispute": d,
                    "escrow": escrow,
                    "client_name": client_name,
                    "freelancer_name": freelancer_name
                }));
            }
            Json(json!({ "status": "ok", "disputes": out }))
        }
        Err(e) => Json(json!({ "status": "error", "message": e.to_string() })),
    }
}

async fn admin_resolve_dispute(
    State(db): State<PgPool>,
    auth_user: auth::AuthUser,
    ExtractJson(payload): ExtractJson<AdminResolveRequest>,
) -> Json<Value> {
    let admin_id = auth_user.user_id.to_string();
    if !is_admin(&db, &admin_id).await {
        return Json(json!({ "status": "error", "message": "Admin access required" }));
    }

    if !(0.0..=100.0).contains(&payload.freelancer_pct) {
        return Json(json!({ "status": "error", "message": "split must be between 0 and 100" }));
    }

    let dispute_uuid = match Uuid::parse_str(&payload.dispute_id) {
        Ok(u) => u,
        Err(_) => return Json(json!({ "status": "error", "message": "invalid dispute id" })),
    };

    let dispute = match sqlx::query_as::<_, SweepDisputeRow>(
        r#"SELECT id, escrow_id, status, last_action_by FROM disputes WHERE id = $1"#,
    )
    .bind(dispute_uuid)
    .fetch_optional(&db)
    .await
    {
        Ok(Some(d)) => d,
        Ok(None) => return Json(json!({ "status": "error", "message": "dispute not found" })),
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    };

    if dispute.status != "escalated" {
        return Json(json!({
            "status": "error",
            "message": format!("only escalated disputes can be resolved by an admin, this one is '{}'", dispute.status)
        }));
    }

    let escrow = match fetch_escrow_row(&db, &dispute.escrow_id.to_string()).await {
        Ok(Some(r)) => Escrow::from(r),
        Ok(None) => return Json(json!({ "status": "error", "message": "escrow not found" })),
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    };

    let update = sqlx::query(
        r#"
        UPDATE disputes
        SET status = 'resolved', resolved_at = now(), final_split_freelancer_pct = $1,
            resolved_by = $2, admin_note = $3
        WHERE id = $4 AND status = 'escalated'
        "#,
    )
    .bind(payload.freelancer_pct)
    .bind(&admin_id)
    .bind(&payload.note)
    .bind(dispute.id)
    .execute(&db)
    .await;

    match update {
        Ok(r) if r.rows_affected() == 0 => {
            return Json(json!({ "status": "error", "message": "dispute was already resolved" }));
        }
        Ok(_) => {}
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    }

    let escrow_uuid = Uuid::parse_str(&escrow.id).unwrap();
    if let Err(e) = sqlx::query(r#"UPDATE escrows SET status = 'confirmed' WHERE id = $1"#)
        .bind(escrow_uuid)
        .execute(&db)
        .await
    {
        return Json(json!({ "status": "error", "message": e.to_string() }));
    }

    if let Ok(Some(client)) = db::find_user_by_id(&db, &escrow.client_id).await {
        email::dispute_resolved(&client.email, &escrow.id).await;
    }
    if let Ok(Some(freelancer)) = db::find_user_by_id(&db, &escrow.freelancer_id).await {
        email::dispute_resolved(&freelancer.email, &escrow.id).await;
    }

    Json(json!({ "status": "resolved", "freelancer_pct": payload.freelancer_pct }))
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

    if escrow.status != "accepted" {
        return Json(json!({
            "status": "error",
            "message": format!("cannot fund an escrow in status '{}'", escrow.status)
        }));
    }

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

    if data.amount != escrow.amount {
        return Json(json!({
            "status": "error",
            "message": format!("amount mismatch: expected {} kobo, Paystack confirms {} kobo", escrow.amount, data.amount)
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

async fn get_escrow(
    State(db): State<PgPool>,
    auth_user: auth::AuthUser,
    Path(id): Path<String>,
) -> Json<Value> {
    match fetch_escrow_row(&db, &id).await {
        Ok(Some(row)) => {
            let escrow: Escrow = row.into();
            let caller_id = auth_user.user_id.to_string();
            let allowed = escrow.client_id == caller_id
                || escrow.freelancer_id == caller_id
                || is_admin(&db, &caller_id).await;
            if !allowed {
                return Json(json!({ "status": "error", "message": "escrow not found" }));
            }
            let dispute = fetch_visible_dispute(&db, &id).await.ok().flatten();
            let extensions = fetch_pending_extensions(&db, &id).await;
            Json(json!({ "status": "ok", "escrow": escrow, "dispute": dispute, "extensions": extensions }))
        }
        Ok(None) => Json(json!({ "status": "error", "message": "escrow not found" })),
        Err(e) => Json(json!({ "status": "error", "message": e.to_string() })),
    }
}

async fn fetch_pending_extensions(db: &PgPool, escrow_id: &str) -> Vec<ExtensionRow> {
    let uuid = match Uuid::parse_str(escrow_id) {
        Ok(u) => u,
        Err(_) => return Vec::new(),
    };

    sqlx::query_as::<_, ExtensionRow>(
        r#"
        SELECT id, escrow_id, milestone_id, requested_by, current_deadline, new_deadline, reason, status
        FROM extension_requests
        WHERE escrow_id = $1 AND status = 'pending'
        ORDER BY created_at ASC
        "#,
    )
    .bind(uuid)
    .fetch_all(db)
    .await
    .unwrap_or_default()
}

async fn request_extension(
    State(db): State<PgPool>,
    auth_user: auth::AuthUser,
    ExtractJson(payload): ExtractJson<RequestExtensionRequest>,
) -> Json<Value> {
    let row = match fetch_escrow_row(&db, &payload.escrow_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Json(json!({ "status": "error", "message": "escrow not found" })),
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    };

    let escrow: Escrow = row.into();
    let caller_id = auth_user.user_id.to_string();

    if escrow.freelancer_id != caller_id {
        return Json(json!({
            "status": "error",
            "message": "Only the freelancer on this escrow can request an extension"
        }));
    }

    if escrow.status != "accepted" && escrow.status != "funded" {
        return Json(json!({
            "status": "error",
            "message": format!("cannot request an extension on an escrow in status '{}'", escrow.status)
        }));
    }

    let milestone = match escrow.milestones.iter().find(|m| m.id == payload.milestone_id) {
        Some(m) => m,
        None => return Json(json!({ "status": "error", "message": "milestone not found" })),
    };

    if milestone.status != "pending" {
        return Json(json!({
            "status": "error",
            "message": "only a milestone that has not been delivered can be extended"
        }));
    }

    let reason = payload.reason.trim().to_string();
    if reason.is_empty() {
        return Json(json!({ "status": "error", "message": "a reason is required" }));
    }
    if reason.chars().count() > 1000 {
        return Json(json!({ "status": "error", "message": "the reason can be 1000 characters at most" }));
    }

    let new_date = match NaiveDate::parse_from_str(&payload.new_deadline, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => return Json(json!({ "status": "error", "message": "new deadline must be a valid date" })),
    };

    if new_date <= chrono::Utc::now().date_naive() {
        return Json(json!({ "status": "error", "message": "new deadline must be in the future" }));
    }

    if let Some(current) = &milestone.deadline {
        if let Ok(current_date) = NaiveDate::parse_from_str(current, "%Y-%m-%d") {
            if new_date <= current_date {
                return Json(json!({
                    "status": "error",
                    "message": "new deadline must be later than the current one"
                }));
            }
        }
    }

    let escrow_uuid = Uuid::parse_str(&escrow.id).unwrap();

    let pending = sqlx::query_scalar::<_, i64>(
        r#"SELECT COUNT(*) FROM extension_requests WHERE escrow_id = $1 AND milestone_id = $2 AND status = 'pending'"#,
    )
    .bind(escrow_uuid)
    .bind(&payload.milestone_id)
    .fetch_one(&db)
    .await;

    match pending {
        Ok(0) => {}
        Ok(_) => {
            return Json(json!({
                "status": "error",
                "message": "there is already a pending extension request for this milestone"
            }));
        }
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    }

    let new_deadline = new_date.format("%Y-%m-%d").to_string();

    let result = sqlx::query(
        r#"
        INSERT INTO extension_requests (escrow_id, milestone_id, requested_by, current_deadline, new_deadline, reason)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(escrow_uuid)
    .bind(&payload.milestone_id)
    .bind(&caller_id)
    .bind(&milestone.deadline)
    .bind(&new_deadline)
    .bind(&reason)
    .execute(&db)
    .await;

    match result {
        Ok(_) => {
            if let Ok(Some(client)) = db::find_user_by_id(&db, &escrow.client_id).await {
                email::extension_requested(&client.email, &escrow.id).await;
            }
            Json(json!({ "status": "requested" }))
        }
        Err(e) => Json(json!({ "status": "error", "message": e.to_string() })),
    }
}

async fn apply_extension_decision(
    db: &PgPool,
    escrow: &mut Escrow,
    ext: &ExtensionRow,
    accept: bool,
) -> Result<bool, sqlx::Error> {
    let mut tx = db.begin().await?;
    let new_status = if accept { "approved" } else { "declined" };

    let updated = sqlx::query(
        r#"UPDATE extension_requests SET status = $1, responded_at = now() WHERE id = $2 AND status = 'pending'"#,
    )
    .bind(new_status)
    .bind(ext.id)
    .execute(&mut *tx)
    .await?;

    if updated.rows_affected() == 0 {
        return Ok(false);
    }

    if accept {
        for m in escrow.milestones.iter_mut() {
            if m.id == ext.milestone_id {
                m.deadline = Some(ext.new_deadline.clone());
            }
        }
        let escrow_uuid = Uuid::parse_str(&escrow.id).unwrap();
        sqlx::query(r#"UPDATE escrows SET milestones = $1 WHERE id = $2"#)
            .bind(SqlxJson(escrow.milestones.clone()))
            .bind(escrow_uuid)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;
    Ok(true)
}

async fn respond_extension(
    State(db): State<PgPool>,
    auth_user: auth::AuthUser,
    ExtractJson(payload): ExtractJson<RespondExtensionRequest>,
) -> Json<Value> {
    let request_uuid = match Uuid::parse_str(&payload.request_id) {
        Ok(u) => u,
        Err(_) => return Json(json!({ "status": "error", "message": "invalid request id" })),
    };

    let ext = match sqlx::query_as::<_, ExtensionRow>(
        r#"
        SELECT id, escrow_id, milestone_id, requested_by, current_deadline, new_deadline, reason, status
        FROM extension_requests WHERE id = $1
        "#,
    )
    .bind(request_uuid)
    .fetch_optional(&db)
    .await
    {
        Ok(Some(e)) => e,
        Ok(None) => return Json(json!({ "status": "error", "message": "extension request not found" })),
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    };

    if ext.status != "pending" {
        return Json(json!({ "status": "error", "message": "this request was already answered" }));
    }

    let mut escrow = match fetch_escrow_row(&db, &ext.escrow_id.to_string()).await {
        Ok(Some(r)) => Escrow::from(r),
        Ok(None) => return Json(json!({ "status": "error", "message": "escrow not found" })),
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    };

    if escrow.client_id != auth_user.user_id.to_string() {
        return Json(json!({
            "status": "error",
            "message": "Only the client on this escrow can respond to an extension request"
        }));
    }

    match apply_extension_decision(&db, &mut escrow, &ext, payload.accept).await {
        Ok(true) => {}
        Ok(false) => {
            return Json(json!({ "status": "error", "message": "this request was already answered" }));
        }
        Err(e) => return Json(json!({ "status": "error", "message": e.to_string() })),
    }

    if let Ok(Some(freelancer)) = db::find_user_by_id(&db, &escrow.freelancer_id).await {
        if payload.accept {
            email::extension_approved(&freelancer.email, &escrow.id).await;
        } else {
            email::extension_declined(&freelancer.email, &escrow.id).await;
        }
    }

    let outcome = if payload.accept { "approved" } else { "declined" };
    Json(json!({ "status": outcome, "escrow": escrow }))
}

async fn user_display_name(db: &PgPool, id: &str) -> String {
    match db::find_user_by_id(db, id).await {
        Ok(Some(u)) => format!("{} {}", u.first_name, u.last_name).trim().to_string(),
        _ => "Unknown user".to_string(),
    }
}

async fn user_name(
    State(db): State<PgPool>,
    _auth_user: auth::AuthUser,
    Path(id): Path<String>,
) -> Json<Value> {
    match db::find_user_by_id(&db, &id).await {
        Ok(Some(u)) => {
            let name = format!("{} {}", u.first_name, u.last_name).trim().to_string();
            Json(json!({ "status": "ok", "name": name }))
        }
        Ok(None) => Json(json!({ "status": "error", "message": "user not found" })),
        Err(e) => Json(json!({ "status": "error", "message": e.to_string() })),
    }
}
