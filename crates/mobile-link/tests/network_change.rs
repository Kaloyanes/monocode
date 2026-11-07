//! Its own test binary: a network change reaches every connection in the
//! process, so it can't run beside the crate's other tests.
use monocode_mobile_link::{
    exchange, network_changed, serve, server_endpoint, watch, Address, Gate, Identity, WatchSink,
};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[tokio::test]
async fn a_network_change_ends_watches_and_waiting_requests_at_once() {
    let desktop = Identity::generate().unwrap();
    let phone = Identity::generate().unwrap();
    let gate = Gate::default();
    gate.allow(phone.fingerprint().unwrap(), None);
    let endpoint = server_endpoint(&desktop, gate.clone(), "127.0.0.1:0".parse().unwrap()).unwrap();
    let port = endpoint.local_addr().unwrap().port();
    let task = tokio::spawn(serve(
        endpoint.clone(),
        gate,
        |_, request: Value| async move {
            if request["slow"] == true {
                tokio::time::sleep(Duration::from_secs(20)).await;
            }
            Ok(request)
        },
        |_, _, sink: WatchSink| async move {
            tokio::spawn(async move { sink.closed().await });
            Ok(())
        },
    ));
    let address = [Address {
        host: "127.0.0.1".into(),
        port,
    }];
    let pin = desktop.fingerprint().unwrap();

    let watching = watch(&phone, &pin, &address, json!({})).await.unwrap();
    let waiting = {
        let (phone, pin, address) = (phone.clone(), pin.clone(), address.clone());
        tokio::spawn(async move { exchange(&phone, &pin, &address, json!({"slow": true})).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;

    let started = Instant::now();
    network_changed();
    assert_eq!(waiting.await.unwrap().unwrap_err(), "The network changed");
    assert_eq!(
        watching.next(Duration::from_secs(5)).await.unwrap_err(),
        "The network changed"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    watching.close().await;

    // The new network works straight away.
    assert_eq!(
        exchange(&phone, &pin, &address, json!(1)).await.unwrap(),
        json!(1)
    );
    endpoint.close(0_u32.into(), b"Test complete");
    task.await.unwrap();
}
