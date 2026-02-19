//! Pushover notification client

use std::sync::Arc;

use async_trait::async_trait;

use crate::config::NotifierConfig;
use crate::io::HttpClient;
use crate::notifier::{Notification, Notifier};

const PUSHOVER_API_URL: &str = "https://api.pushover.net/1/messages.json";

/// Pushover notification sender
pub struct PushoverNotifier {
    api_token: String,
    user_key: String,
    default_title: String,
    default_priority: i8,
    default_sound: String,
    http: Arc<dyn HttpClient>,
}

impl std::fmt::Debug for PushoverNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushoverNotifier")
            .field("default_title", &self.default_title)
            .finish()
    }
}

impl PushoverNotifier {
    pub fn new(config: &NotifierConfig, http: Arc<dyn HttpClient>) -> Self {
        let NotifierConfig::Pushover {
            api_token,
            user_key,
            default_title,
            default_priority,
            default_sound,
        } = config;

        tracing::debug!("Created PushoverNotifier with title '{}'", default_title);

        Self {
            api_token: api_token.clone(),
            user_key: user_key.clone(),
            default_title: default_title.clone(),
            default_priority: *default_priority,
            default_sound: default_sound.clone(),
            http,
        }
    }
}

#[async_trait]
impl Notifier for PushoverNotifier {
    fn type_name(&self) -> &str {
        "pushover"
    }

    async fn notify(&self, notification: &Notification) -> crate::Result<()> {
        let title = if notification.title.is_empty() {
            &self.default_title
        } else {
            &notification.title
        };
        let priority = if notification.priority != 0 {
            notification.priority
        } else {
            self.default_priority
        };
        let sound = notification.sound.as_deref().unwrap_or(&self.default_sound);

        let priority_str = priority.to_string();
        let params = vec![
            ("token", self.api_token.as_str()),
            ("user", self.user_key.as_str()),
            ("title", title),
            ("message", notification.message.as_str()),
            ("priority", &priority_str),
            ("sound", sound),
        ];

        tracing::debug!(
            "Sending Pushover notification: title='{}', priority={}",
            title,
            priority
        );

        let response = self.http.post_form(PUSHOVER_API_URL, &params).await?;

        if response.status != 200 {
            return Err(crate::SentinelError::Notifier(format!(
                "Pushover API returned status {}: {}",
                response.status, response.body
            )));
        }

        tracing::debug!("Pushover notification sent successfully");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{HttpResponse, MockHttpClient};

    fn test_config() -> NotifierConfig {
        NotifierConfig::Pushover {
            api_token: "test-token".to_string(),
            user_key: "test-user".to_string(),
            default_title: "Test Alert".to_string(),
            default_priority: 0,
            default_sound: "pushover".to_string(),
        }
    }

    fn test_notification() -> Notification {
        Notification {
            title: "Alert".to_string(),
            message: "Something happened".to_string(),
            priority: 1,
            sound: Some("siren".to_string()),
        }
    }

    #[tokio::test]
    async fn sends_notification_with_correct_params() {
        let mut mock = MockHttpClient::new();
        mock.expect_post_form()
            .withf(|url, params| {
                url == PUSHOVER_API_URL
                    && params.contains(&("token", "test-token"))
                    && params.contains(&("user", "test-user"))
                    && params.contains(&("title", "Alert"))
                    && params.contains(&("message", "Something happened"))
                    && params.contains(&("priority", "1"))
                    && params.contains(&("sound", "siren"))
            })
            .returning(|_, _| {
                Box::pin(async {
                    Ok(HttpResponse {
                        status: 200,
                        body: r#"{"status":1}"#.to_string(),
                    })
                })
            });

        let notifier = PushoverNotifier::new(&test_config(), Arc::new(mock));
        notifier.notify(&test_notification()).await.unwrap();
    }

    #[tokio::test]
    async fn uses_defaults_when_notification_has_empty_title() {
        let mut mock = MockHttpClient::new();
        mock.expect_post_form()
            .withf(|_, params| {
                params.contains(&("title", "Test Alert"))
                    && params.contains(&("priority", "0"))
                    && params.contains(&("sound", "pushover"))
            })
            .returning(|_, _| {
                Box::pin(async {
                    Ok(HttpResponse {
                        status: 200,
                        body: r#"{"status":1}"#.to_string(),
                    })
                })
            });

        let notifier = PushoverNotifier::new(&test_config(), Arc::new(mock));
        let notification = Notification {
            title: "".to_string(),
            message: "msg".to_string(),
            priority: 0,
            sound: None,
        };
        notifier.notify(&notification).await.unwrap();
    }

    #[tokio::test]
    async fn returns_error_on_non_200() {
        let mut mock = MockHttpClient::new();
        mock.expect_post_form().returning(|_, _| {
            Box::pin(async {
                Ok(HttpResponse {
                    status: 400,
                    body: r#"{"status":0,"errors":["invalid token"]}"#.to_string(),
                })
            })
        });

        let notifier = PushoverNotifier::new(&test_config(), Arc::new(mock));
        let err = notifier.notify(&test_notification()).await.unwrap_err();
        assert!(err.to_string().contains("400"));
    }

    #[tokio::test]
    async fn returns_error_on_http_failure() {
        let mut mock = MockHttpClient::new();
        mock.expect_post_form().returning(|_, _| {
            Box::pin(async { Err(crate::SentinelError::Http("timeout".to_string())) })
        });

        let notifier = PushoverNotifier::new(&test_config(), Arc::new(mock));
        let err = notifier.notify(&test_notification()).await.unwrap_err();
        assert!(err.to_string().contains("timeout"));
    }

    #[tokio::test]
    async fn type_name_is_pushover() {
        let mock = MockHttpClient::new();
        let notifier = PushoverNotifier::new(&test_config(), Arc::new(mock));
        assert_eq!(notifier.type_name(), "pushover");
    }
}
