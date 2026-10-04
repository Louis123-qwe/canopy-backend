use serde_json::json;

fn wrap_template(heading: &str, body_html: &str, cta_label: &str, cta_url: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<body style="margin:0;padding:0;background-color:#EDE8DA;font-family:Georgia,serif;">
  <table width="100%" cellpadding="0" cellspacing="0" style="background-color:#EDE8DA;padding:32px 0;">
    <tr>
      <td align="center">
        <table width="420" cellpadding="0" cellspacing="0" style="background-color:#FAF8F1;border-radius:12px;overflow:hidden;">
          <tr>
            <td style="padding:28px 32px 0;">
              <div style="width:36px;height:36px;border-radius:50%;background:#1B4D3E;display:inline-block;text-align:center;line-height:36px;color:#FAF8F1;font-weight:bold;font-size:18px;">C</div>
              <span style="font-family:Georgia,serif;font-size:18px;color:#1B4D3E;font-weight:600;margin-left:8px;">Canopy</span>
            </td>
          </tr>
          <tr>
            <td style="padding:24px 32px 8px;">
              <h1 style="font-family:Georgia,serif;font-size:22px;color:#1B4D3E;margin:0 0 12px;">{heading}</h1>
              <div style="font-family:Arial,sans-serif;font-size:15px;color:#333;line-height:1.6;">{body}</div>
            </td>
          </tr>
          <tr>
            <td style="padding:24px 32px 32px;">
              <a href="{url}" style="display:inline-block;background-color:#1B4D3E;color:#FAF8F1;text-decoration:none;padding:14px 28px;border-radius:8px;font-family:Arial,sans-serif;font-size:15px;font-weight:600;">{cta}</a>
            </td>
          </tr>
          <tr>
            <td style="padding:0 32px 28px;">
              <p style="font-family:Arial,sans-serif;font-size:12px;color:#999;margin:0;">Canopy — escrow that holds up its end.</p>
            </td>
          </tr>
        </table>
      </td>
    </tr>
  </table>
</body>
</html>"#,
        heading = heading,
        body = body_html,
        url = cta_url,
        cta = cta_label
    )
}

async fn send_email(to: &str, subject: &str, html: String) {
    let api_key = match std::env::var("RESEND_API_KEY") {
        Ok(k) => k,
        Err(_) => {
            eprintln!("RESEND_API_KEY not set, skipping email to {to}");
            return;
        }
    };

    let from = std::env::var("EMAIL_FROM")
        .unwrap_or_else(|_| "Canopy <onboarding@resend.dev>".to_string());

    let client = reqwest::Client::new();
    let result = client
        .post("https://api.resend.com/emails")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&json!({
            "from": from,
            "to": [to],
            "subject": subject,
            "html": html
        }))
        .send()
        .await;

    match result {
        Ok(resp) if resp.status().is_success() => {}
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            eprintln!("Resend rejected email to {to}: {status} {body}");
        }
        Err(e) => eprintln!("Failed to send email to {to}: {e}"),
    }
}

fn app_url(escrow_id: &str) -> String {
    format!("https://canopy-lime.vercel.app/#escrow/{}", escrow_id)
}

pub async fn proposal_sent(client_email: &str, freelancer_name: &str, escrow_id: &str) {
    let body = format!("{} sent you an escrow proposal on Canopy. Review the terms and respond.", freelancer_name);
    let html = wrap_template("You have a new proposal", &body, "Review proposal", &app_url(escrow_id));
    send_email(client_email, "New escrow proposal on Canopy", html).await;
}

pub async fn proposal_accepted(freelancer_email: &str, escrow_id: &str) {
    let body = "Your proposal was accepted. The client can now fund the escrow.".to_string();
    let html = wrap_template("Proposal accepted", &body, "View escrow", &app_url(escrow_id));
    send_email(freelancer_email, "Your proposal was accepted", html).await;
}

pub async fn proposal_rejected(freelancer_email: &str, escrow_id: &str) {
    let body = "The client declined this proposal. You can edit the terms and resend.".to_string();
    let html = wrap_template("Proposal declined", &body, "Edit and resend", &app_url(escrow_id));
    send_email(freelancer_email, "Your proposal was declined", html).await;
}

pub async fn milestone_delivered(client_email: &str, escrow_id: &str) {
    let body = "A milestone has been marked delivered. Confirm within 72 hours, or it will auto-confirm.".to_string();
    let html = wrap_template("Milestone delivered", &body, "Review and confirm", &app_url(escrow_id));
    send_email(client_email, "A milestone is ready for your review", html).await;
}

pub async fn milestone_confirmed(freelancer_email: &str, escrow_id: &str) {
    let body = "Your milestone was confirmed. Funds for this milestone are now released.".to_string();
    let html = wrap_template("Milestone confirmed", &body, "View escrow", &app_url(escrow_id));
    send_email(freelancer_email, "Milestone confirmed", html).await;
}

pub async fn extension_requested(client_email: &str, escrow_id: &str) {
    let body = "The freelancer asked for more time on a milestone. Review the new date and the reason, then approve or decline.".to_string();
    let html = wrap_template("Extension requested", &body, "Review request", &app_url(escrow_id));
    send_email(client_email, "A deadline extension needs your response", html).await;
}

pub async fn extension_approved(freelancer_email: &str, escrow_id: &str) {
    let body = "The client approved your extension request. The milestone deadline has been updated.".to_string();
    let html = wrap_template("Extension approved", &body, "View escrow", &app_url(escrow_id));
    send_email(freelancer_email, "Your extension request was approved", html).await;
}

pub async fn extension_declined(freelancer_email: &str, escrow_id: &str) {
    let body = "The client declined your extension request. The original deadline still applies.".to_string();
    let html = wrap_template("Extension declined", &body, "View escrow", &app_url(escrow_id));
    send_email(freelancer_email, "Your extension request was declined", html).await;
}

pub async fn dispute_raised(freelancer_email: &str, escrow_id: &str) {
    let body = "A dispute was raised on this escrow. You have 96 hours to respond.".to_string();
    let html = wrap_template("A dispute was raised", &body, "Respond now", &app_url(escrow_id));
    send_email(freelancer_email, "A dispute needs your response", html).await;
}

pub async fn dispute_countered(client_email: &str, escrow_id: &str) {
    let body = "The freelancer sent a counter-offer on this dispute.".to_string();
    let html = wrap_template("Counter-offer received", &body, "Respond now", &app_url(escrow_id));
    send_email(client_email, "A counter-offer needs your response", html).await;
}

pub async fn dispute_reminder(email: &str, escrow_id: &str) {
    let body = "You have 24 hours left to respond to an open dispute before it escalates.".to_string();
    let html = wrap_template("24 hours left to respond", &body, "Respond now", &app_url(escrow_id));
    send_email(email, "Reminder: dispute response needed", html).await;
}

pub async fn dispute_escalated(email: &str, escrow_id: &str) {
    let body = "No agreement was reached. This dispute is now with Canopy for review.".to_string();
    let html = wrap_template("Dispute escalated", &body, "View escrow", &app_url(escrow_id));
    send_email(email, "Dispute escalated to Canopy", html).await;
}

pub async fn dispute_resolved(email: &str, escrow_id: &str) {
    let body = "This dispute has been resolved and funds have been released accordingly.".to_string();
    let html = wrap_template("Dispute resolved", &body, "View escrow", &app_url(escrow_id));
    send_email(email, "Your dispute has been resolved", html).await;
}
