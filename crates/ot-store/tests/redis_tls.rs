//! TLS to Redis, against a server from `OT_TEST_REDIS_TLS_URL` (rediss://)
//! with its CA and a client certificate in `OT_TEST_REDIS_TLS_DIR`
//! (ca.pem, cli.pem, cli.key). Skipped when unset.

#[tokio::test]
async fn connects_over_mutual_tls() {
    let (Ok(url), Ok(dir)) = (
        std::env::var("OT_TEST_REDIS_TLS_URL"),
        std::env::var("OT_TEST_REDIS_TLS_DIR"),
    ) else {
        return;
    };
    let _ = rustls::crypto::default_fips_provider().install_default();
    let read = |f: &str| std::fs::read(format!("{dir}/{f}")).unwrap();
    let client = redis::Client::build_with_tls(
        url.as_str(),
        redis::TlsCertificates {
            client_tls: Some(redis::ClientTlsConfig {
                client_cert: read("cli.pem"),
                client_key: read("cli.key"),
            }),
            root_cert: Some(read("ca.pem")),
        },
    )
    .unwrap();
    let mut c = client.get_multiplexed_async_connection().await.unwrap();
    let pong: String = redis::cmd("PING").query_async(&mut c).await.unwrap();
    assert_eq!(pong, "PONG");
}
