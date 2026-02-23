//! Notification history table component

use crate::api::NotificationHistoryResponse;
use leptos::prelude::*;

/// Fetches /api/history and displays notification history in a table
#[component]
pub fn HistoryTable() -> impl IntoView {
    let history = Resource::new(
        || (),
        |_| async move { fetch_history().await.unwrap_or_default() },
    );

    view! {
        <section>
            <h2>"Notification History"</h2>
            <Suspense fallback=move || view! { <p>"Loading history..."</p> }>
                {move || {
                    history.get().map(|data| {
                        if data.is_empty() {
                            view! { <p>"No notifications yet."</p> }.into_any()
                        } else {
                            view! {
                                <table style="width: 100%; border-collapse: collapse;">
                                    <thead>
                                        <tr style="border-bottom: 2px solid #dee2e6;">
                                            <th style="padding: 0.5rem; text-align: left;">"Monitor"</th>
                                            <th style="padding: 0.5rem; text-align: left;">"Message"</th>
                                            <th style="padding: 0.5rem; text-align: left;">"Notifier"</th>
                                            <th style="padding: 0.5rem; text-align: left;">"Status"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {data.into_iter().map(|h| {
                                            let status = if h.success { "OK" } else { "Failed" };
                                            view! {
                                                <tr style="border-bottom: 1px solid #dee2e6;">
                                                    <td style="padding: 0.5rem;">{h.monitor_name}</td>
                                                    <td style="padding: 0.5rem;">{h.message}</td>
                                                    <td style="padding: 0.5rem;">{h.notifier_type}</td>
                                                    <td style="padding: 0.5rem;">{status}</td>
                                                </tr>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </tbody>
                                </table>
                            }.into_any()
                        }
                    })
                }}
            </Suspense>
        </section>
    }
}

async fn fetch_history() -> Result<Vec<NotificationHistoryResponse>, String> {
    #[cfg(all(feature = "hydrate", target_arch = "wasm32"))]
    {
        let window = web_sys::window().ok_or("no window")?;
        let origin = window.location().origin().map_err(|e| format!("{:?}", e))?;
        let url = format!("{}/api/history", origin);

        let resp = gloo_net::http::Request::get(&url)
            .send()
            .await
            .map_err(|e| format!("{}", e))?;

        resp.json().await.map_err(|e| format!("{}", e))
    }

    #[cfg(not(all(feature = "hydrate", target_arch = "wasm32")))]
    {
        Ok(vec![])
    }
}
