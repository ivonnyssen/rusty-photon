//! BDD step definitions for notification feature

use std::sync::Arc;

use cucumber::{given, then, when};

use sentinel::config::NotifierConfig;
use sentinel::io::{HttpClient, HttpResponse};
use sentinel::notifier::Notification;
use sentinel::pushover::PushoverNotifier;
use sentinel::SentinelError;

use crate::world::SentinelWorld;

fn test_notifier_config() -> NotifierConfig {
    NotifierConfig::Pushover {
        api_token: "test-token".to_string(),
        user_key: "test-user".to_string(),
        default_title: "Default Alert".to_string(),
        default_priority: 0,
        default_sound: "pushover".to_string(),
    }
}

/// Mock HTTP client for Pushover API that succeeds
struct SuccessPostClient;

#[async_trait::async_trait]
impl HttpClient for SuccessPostClient {
    async fn get(&self, _url: &str) -> sentinel::Result<HttpResponse> {
        Ok(HttpResponse {
            status: 200,
            body: "{}".to_string(),
        })
    }

    async fn put_form(
        &self,
        _url: &str,
        _params: &[(&str, &str)],
    ) -> sentinel::Result<HttpResponse> {
        Ok(HttpResponse {
            status: 200,
            body: "{}".to_string(),
        })
    }

    async fn post_form(
        &self,
        _url: &str,
        _params: &[(&str, &str)],
    ) -> sentinel::Result<HttpResponse> {
        Ok(HttpResponse {
            status: 200,
            body: r#"{"status":1}"#.to_string(),
        })
    }
}

/// Mock HTTP client that returns a Pushover API error
struct ErrorPostClient;

#[async_trait::async_trait]
impl HttpClient for ErrorPostClient {
    async fn get(&self, _url: &str) -> sentinel::Result<HttpResponse> {
        Ok(HttpResponse {
            status: 200,
            body: "{}".to_string(),
        })
    }

    async fn put_form(
        &self,
        _url: &str,
        _params: &[(&str, &str)],
    ) -> sentinel::Result<HttpResponse> {
        Ok(HttpResponse {
            status: 200,
            body: "{}".to_string(),
        })
    }

    async fn post_form(
        &self,
        _url: &str,
        _params: &[(&str, &str)],
    ) -> sentinel::Result<HttpResponse> {
        Ok(HttpResponse {
            status: 400,
            body: r#"{"status":0,"errors":["invalid token"]}"#.to_string(),
        })
    }
}

/// Mock HTTP client that simulates network failure
struct UnreachablePostClient;

#[async_trait::async_trait]
impl HttpClient for UnreachablePostClient {
    async fn get(&self, _url: &str) -> sentinel::Result<HttpResponse> {
        Err(SentinelError::Http("connection refused".to_string()))
    }

    async fn put_form(
        &self,
        _url: &str,
        _params: &[(&str, &str)],
    ) -> sentinel::Result<HttpResponse> {
        Err(SentinelError::Http("connection refused".to_string()))
    }

    async fn post_form(
        &self,
        _url: &str,
        _params: &[(&str, &str)],
    ) -> sentinel::Result<HttpResponse> {
        Err(SentinelError::Http("connection refused".to_string()))
    }
}

#[given("a Pushover notifier with valid credentials")]
fn pushover_valid(world: &mut SentinelWorld) {
    let notifier = PushoverNotifier::new(&test_notifier_config(), Arc::new(SuccessPostClient));
    world.notifier = Some(Box::new(notifier));
}

#[given("a Pushover notifier that returns an API error")]
fn pushover_api_error(world: &mut SentinelWorld) {
    let notifier = PushoverNotifier::new(&test_notifier_config(), Arc::new(ErrorPostClient));
    world.notifier = Some(Box::new(notifier));
}

#[given("a Pushover notifier that is unreachable")]
fn pushover_unreachable(world: &mut SentinelWorld) {
    let notifier = PushoverNotifier::new(&test_notifier_config(), Arc::new(UnreachablePostClient));
    world.notifier = Some(Box::new(notifier));
}

#[when(expr = "a notification is sent with title {string} and message {string}")]
async fn send_notification(world: &mut SentinelWorld, title: String, message: String) {
    let notifier = world.notifier.as_ref().expect("notifier not set");
    let notification = Notification {
        title,
        message,
        priority: 0,
        sound: None,
    };
    world.notification_result = Some(notifier.notify(&notification).await);
}

#[then("the notification should succeed")]
fn notification_succeeds(world: &mut SentinelWorld) {
    let result = world.notification_result.as_ref().expect("no result");
    result.as_ref().unwrap();
}

#[then("the notification should fail with an error")]
fn notification_fails(world: &mut SentinelWorld) {
    let result = world.notification_result.as_ref().expect("no result");
    assert!(result.is_err());
}
