use ale_core::mock_server::{MockBehavior, MockRemoteServer};

#[tokio::main]
async fn main() {
    let server = MockRemoteServer::start(
        ale_core::remote::REMOTE_PROTOCOL_VERSION,
        MockBehavior::Normal,
    )
    .await
    .expect("start mock remote v2 server");
    println!("{}", server.pairing.uri());
    std::future::pending::<()>().await;
}
