//! Notification ports: capture senders, host registration, and the
//! "no silent no-op" rule.
//!
//! The framework registers no email/SMS sender by default. Tests and local
//! development register the in-memory capture senders; production registers an
//! SMTP/API adapter. A handler depends on the port, so swapping the adapter
//! never changes business code.

use std::sync::Arc;

use webx::*;

#[tokio::test]
async fn a_host_registers_the_email_and_sms_ports_and_they_capture() {
    let email = Arc::new(InMemoryEmailSender::new());
    let sms = Arc::new(InMemorySmsSender::new());

    let host = Host::builder()
        .mode(AppMode::Development)
        .no_spa()
        .use_email(email.clone())
        .use_sms(sms.clone())
        .build();

    let sender = host
        .provider()
        .get::<dyn IEmailSender>()
        .expect("the email port is registered");
    sender
        .send(&EmailMessage::new(
            "alice@example.com",
            "Reset your password",
            "open the link",
        ))
        .await
        .expect("captured");
    assert_eq!(email.len(), 1);
    assert_eq!(
        email.last().expect("captured").subject,
        "Reset your password"
    );

    let sms_sender = host
        .provider()
        .get::<dyn ISmsSender>()
        .expect("the sms port is registered");
    sms_sender
        .send(&SmsMessage::new("+15550100", "code 1234"))
        .await
        .expect("captured");
    assert_eq!(sms.sent().len(), 1);
}

#[tokio::test]
async fn a_host_without_an_adapter_does_not_install_a_silent_noop() {
    let host = Host::builder().mode(AppMode::Development).no_spa().build();

    assert!(
        host.provider().get::<dyn IEmailSender>().is_err(),
        "a missing email adapter must fail at resolution, not drop mail"
    );
    assert!(
        host.provider().get::<dyn ISmsSender>().is_err(),
        "a missing sms adapter must fail at resolution"
    );
}

#[tokio::test]
async fn capture_senders_enforce_the_message_contract() {
    let email = Arc::new(InMemoryEmailSender::new());
    let error = email
        .send(&EmailMessage::new("not-an-email", "subject", "body"))
        .await
        .expect_err("invalid recipient");
    assert_eq!(error.status_code(), 400);
    assert!(email.is_empty());

    let sms = Arc::new(InMemorySmsSender::new());
    let error = sms
        .send(&SmsMessage::new("", "code"))
        .await
        .expect_err("invalid destination");
    assert_eq!(error.status_code(), 400);
}
