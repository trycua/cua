//! `FleetClient::usage_pricing` against the in-memory fake Fleet.

use cua_fleet::testing::FakeFleet;

fn config_reads(fake: &FakeFleet) -> usize {
    fake.requests()
        .iter()
        .filter(|r| r.path == "/api/config")
        .count()
}

#[tokio::test]
async fn reads_the_callers_rates_and_reuses_them() {
    let fake = FakeFleet::new();
    fake.faults.lock().unwrap().usage_pricing = Some((0.044625, 0.0223125));
    // A base of its own: the cache is per base URL and process-wide.
    let fleet = fake.client_with_base("https://pricing-reuse.fleet.test");
    let p = fleet.usage_pricing().await.unwrap().expect("rates");
    assert_eq!(p.vcpu_hour_usd, 0.044625);
    assert_eq!(p.memory_gib_hour_usd, 0.0223125);
    assert!((p.hourly_usd(2, 4096) - 0.1785).abs() < 1e-12);
    fleet.usage_pricing().await.unwrap();
    assert_eq!(config_reads(&fake), 1, "the second read is cached");
}

#[tokio::test]
async fn no_rates_is_none_and_is_asked_again() {
    let fake = FakeFleet::new();
    let fleet = fake.client_with_base("https://pricing-none.fleet.test");
    assert_eq!(fleet.usage_pricing().await.unwrap(), None);
    fake.faults.lock().unwrap().usage_pricing = Some((0.05, 0.02));
    let p = fleet.usage_pricing().await.unwrap().expect("rates now");
    assert_eq!(p.vcpu_hour_usd, 0.05);
    assert_eq!(config_reads(&fake), 2, "a missing answer is not cached");
}
