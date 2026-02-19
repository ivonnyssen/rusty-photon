//! Web dashboard with JSON API endpoints and Leptos SSR

use axum::extract::State;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;

use crate::state::StateHandle;

/// Dashboard application state
#[derive(Clone)]
pub struct DashboardState {
    pub state: StateHandle,
}

/// Build the dashboard axum router
pub fn build_router(state: StateHandle) -> Router {
    let dashboard_state = DashboardState { state };

    Router::new()
        .route("/", get(index_handler))
        .route("/api/status", get(status_handler))
        .route("/api/history", get(history_handler))
        .route("/health", get(health_handler))
        .with_state(dashboard_state)
}

async fn index_handler(State(dashboard): State<DashboardState>) -> impl IntoResponse {
    let state = dashboard.state.read().await;

    let monitor_rows: String = state
        .monitors
        .iter()
        .map(|m| {
            let (color, bg) = match m.state {
                crate::monitor::MonitorState::Safe => ("#155724", "#d4edda"),
                crate::monitor::MonitorState::Unsafe => ("#721c24", "#f8d7da"),
                crate::monitor::MonitorState::Unknown => ("#383d41", "#e2e3e5"),
            };
            let last_check = if m.last_poll_epoch_ms == 0 {
                "Never".to_string()
            } else {
                format!(
                    r#"<script>document.write(new Date({}).toLocaleTimeString())</script>"#,
                    m.last_poll_epoch_ms
                )
            };
            let next_check = if m.last_poll_epoch_ms == 0 {
                "Pending".to_string()
            } else {
                format!(
                    r#"<script>document.write(new Date({}).toLocaleTimeString())</script>"#,
                    m.last_poll_epoch_ms + m.polling_interval_ms
                )
            };
            format!(
                r#"<tr style="border-bottom: 1px solid #dee2e6;">
                    <td style="padding: 0.5rem;">{}</td>
                    <td style="padding: 0.5rem;">
                        <span style="display: inline-block; padding: 0.25em 0.6em; border-radius: 0.25rem; font-size: 0.85em; font-weight: 600; color: {}; background-color: {};">{}</span>
                    </td>
                    <td style="padding: 0.5rem;">{}</td>
                    <td style="padding: 0.5rem;">{}</td>
                    <td style="padding: 0.5rem;">{}</td>
                </tr>"#,
                m.name, color, bg, m.state, m.consecutive_errors, last_check, next_check
            )
        })
        .collect();

    let history_rows: String = state
        .history
        .iter()
        .rev()
        .map(|h| {
            let status = if h.success { "OK" } else { "Failed" };
            format!(
                r#"<tr style="border-bottom: 1px solid #dee2e6;">
                    <td style="padding: 0.5rem;">{}</td>
                    <td style="padding: 0.5rem;">{}</td>
                    <td style="padding: 0.5rem;">{}</td>
                    <td style="padding: 0.5rem;">{}</td>
                </tr>"#,
                h.monitor_name, h.message, h.notifier_type, status
            )
        })
        .collect();

    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Sentinel Dashboard</title>
    <script>
        function refreshData() {{
            fetch('/api/status')
                .then(r => r.json())
                .then(data => {{
                    const tbody = document.getElementById('monitor-body');
                    tbody.innerHTML = data.map(m => {{
                        const colors = {{
                            'Safe': ['#155724', '#d4edda'],
                            'Unsafe': ['#721c24', '#f8d7da'],
                        }};
                        const [color, bg] = colors[m.state] || ['#383d41', '#e2e3e5'];
                        const lastCheck = m.last_poll_epoch_ms === 0 ? 'Never' : new Date(m.last_poll_epoch_ms).toLocaleTimeString();
                        const nextCheck = m.last_poll_epoch_ms === 0 ? 'Pending' : new Date(m.last_poll_epoch_ms + m.polling_interval_ms).toLocaleTimeString();
                        return `<tr style="border-bottom: 1px solid #dee2e6;">
                            <td style="padding: 0.5rem;">${{m.name}}</td>
                            <td style="padding: 0.5rem;">
                                <span style="display: inline-block; padding: 0.25em 0.6em; border-radius: 0.25rem; font-size: 0.85em; font-weight: 600; color: ${{color}}; background-color: ${{bg}};">${{m.state}}</span>
                            </td>
                            <td style="padding: 0.5rem;">${{m.consecutive_errors}}</td>
                            <td style="padding: 0.5rem;">${{lastCheck}}</td>
                            <td style="padding: 0.5rem;">${{nextCheck}}</td>
                        </tr>`;
                    }}).join('');
                }});
            fetch('/api/history')
                .then(r => r.json())
                .then(data => {{
                    const tbody = document.getElementById('history-body');
                    tbody.innerHTML = data.reverse().map(h => {{
                        const status = h.success ? 'OK' : 'Failed';
                        return `<tr style="border-bottom: 1px solid #dee2e6;">
                            <td style="padding: 0.5rem;">${{h.monitor_name}}</td>
                            <td style="padding: 0.5rem;">${{h.message}}</td>
                            <td style="padding: 0.5rem;">${{h.notifier_type}}</td>
                            <td style="padding: 0.5rem;">${{status}}</td>
                        </tr>`;
                    }}).join('');
                }});
        }}
        setInterval(refreshData, 5000);
    </script>
</head>
<body style="font-family: system-ui, sans-serif; max-width: 960px; margin: 0 auto; padding: 1rem;">
    <h1>Sentinel Dashboard</h1>
    <section>
        <h2>Monitors</h2>
        <table style="width: 100%; border-collapse: collapse;">
            <thead>
                <tr style="border-bottom: 2px solid #dee2e6;">
                    <th style="padding: 0.5rem; text-align: left;">Name</th>
                    <th style="padding: 0.5rem; text-align: left;">State</th>
                    <th style="padding: 0.5rem; text-align: left;">Errors</th>
                    <th style="padding: 0.5rem; text-align: left;">Last Check</th>
                    <th style="padding: 0.5rem; text-align: left;">Next Check</th>
                </tr>
            </thead>
            <tbody id="monitor-body">{monitor_rows}</tbody>
        </table>
    </section>
    <section>
        <h2>Notification History</h2>
        <table style="width: 100%; border-collapse: collapse;">
            <thead>
                <tr style="border-bottom: 2px solid #dee2e6;">
                    <th style="padding: 0.5rem; text-align: left;">Monitor</th>
                    <th style="padding: 0.5rem; text-align: left;">Message</th>
                    <th style="padding: 0.5rem; text-align: left;">Notifier</th>
                    <th style="padding: 0.5rem; text-align: left;">Status</th>
                </tr>
            </thead>
            <tbody id="history-body">{history_rows}</tbody>
        </table>
    </section>
</body>
</html>"#,
        monitor_rows = monitor_rows,
        history_rows = history_rows,
    );

    Html(html)
}

async fn status_handler(State(dashboard): State<DashboardState>) -> impl IntoResponse {
    let state = dashboard.state.read().await;

    let statuses: Vec<serde_json::Value> = state
        .monitors
        .iter()
        .map(|m| {
            serde_json::json!({
                "name": m.name,
                "state": format!("{}", m.state),
                "last_poll_epoch_ms": m.last_poll_epoch_ms,
                "last_change_epoch_ms": m.last_change_epoch_ms,
                "consecutive_errors": m.consecutive_errors,
                "polling_interval_ms": m.polling_interval_ms,
            })
        })
        .collect();

    axum::Json(statuses)
}

async fn history_handler(State(dashboard): State<DashboardState>) -> impl IntoResponse {
    let state = dashboard.state.read().await;

    let history: Vec<serde_json::Value> = state
        .history
        .iter()
        .map(|h| {
            serde_json::json!({
                "monitor_name": h.monitor_name,
                "notifier_type": h.notifier_type,
                "message": h.message,
                "success": h.success,
                "error": h.error,
                "timestamp_epoch_ms": h.timestamp_epoch_ms,
            })
        })
        .collect();

    axum::Json(history)
}

async fn health_handler() -> impl IntoResponse {
    "OK"
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::monitor::MonitorState;
    use crate::notifier::NotificationRecord;
    use crate::state::new_state_handle;

    fn setup_state() -> StateHandle {
        new_state_handle(vec![("Test Monitor".to_string(), 30000)], 10)
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let state = setup_state();
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn status_returns_json() {
        let state = setup_state();
        {
            let mut s = state.write().await;
            s.update_monitor("Test Monitor", MonitorState::Safe, 1000);
        }
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["name"], "Test Monitor");
        assert_eq!(json[0]["state"], "Safe");
        assert_eq!(json[0]["polling_interval_ms"], 30000);
    }

    #[tokio::test]
    async fn history_returns_json() {
        let state = setup_state();
        {
            let mut s = state.write().await;
            s.add_notification(NotificationRecord {
                monitor_name: "Test Monitor".to_string(),
                notifier_type: "pushover".to_string(),
                message: "alert".to_string(),
                success: true,
                error: None,
                timestamp_epoch_ms: 1000,
            });
        }
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/history")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["monitor_name"], "Test Monitor");
        assert!(json[0]["success"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn index_returns_html() {
        let state = setup_state();
        let app = build_router(state);
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Sentinel Dashboard"));
        assert!(html.contains("Last Check"));
        assert!(html.contains("Next Check"));
    }

    #[tokio::test]
    async fn status_empty_monitors() {
        let state = new_state_handle(vec![], 10);
        let app = build_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap();
        assert!(json.is_empty());
    }
}
