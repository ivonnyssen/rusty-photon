//! Monitor status table component

use crate::api::MonitorStatusResponse;
use crate::components::status_badge::StatusBadge;
use leptos::prelude::*;

/// Fetches /api/status and displays monitor states in a table
#[component]
pub fn MonitorTable() -> impl IntoView {
    let monitors = Resource::new(
        || (),
        |_| async move { fetch_monitors().await.unwrap_or_default() },
    );

    view! {
        <section>
            <h2>"Monitors"</h2>
            <Suspense fallback=move || view! { <p>"Loading monitors..."</p> }>
                {move || {
                    monitors.get().map(|data| {
                        if data.is_empty() {
                            view! { <p>"No monitors configured."</p> }.into_any()
                        } else {
                            view! {
                                <table style="width: 100%; border-collapse: collapse;">
                                    <thead>
                                        <tr style="border-bottom: 2px solid #dee2e6;">
                                            <th style="padding: 0.5rem; text-align: left;">"Name"</th>
                                            <th style="padding: 0.5rem; text-align: left;">"State"</th>
                                            <th style="padding: 0.5rem; text-align: left;">"Errors"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {data.into_iter().map(|m| {
                                            view! {
                                                <tr style="border-bottom: 1px solid #dee2e6;">
                                                    <td style="padding: 0.5rem;">{m.name}</td>
                                                    <td style="padding: 0.5rem;">
                                                        <StatusBadge state=m.state />
                                                    </td>
                                                    <td style="padding: 0.5rem;">{m.consecutive_errors}</td>
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

async fn fetch_monitors() -> Result<Vec<MonitorStatusResponse>, String> {
    // In SSR mode, this returns empty (server populates via initial state)
    // In hydrate/CSR mode, this fetches from the JSON API
    #[cfg(feature = "hydrate")]
    {
        let window = web_sys::window().ok_or("no window")?;
        let origin = window.location().origin().map_err(|e| format!("{:?}", e))?;
        let url = format!("{}/api/status", origin);

        let resp = gloo_net::http::Request::get(&url)
            .send()
            .await
            .map_err(|e| format!("{}", e))?;

        resp.json().await.map_err(|e| format!("{}", e))
    }

    #[cfg(not(feature = "hydrate"))]
    {
        Ok(vec![])
    }
}
