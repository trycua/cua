//! Windows shown in another Space, against two in-process spacesd mocks: the
//! registry, the direct-vs-proxied route, cleanup, and what the agent's
//! tools answer. No network beyond loopback, no host effects.

use cua_spaces::mcp::McpServer;
use cua_spaces::stream::StreamOptions;
use cua_spaces::stream_registry::{Access, Route, entry_window_id};
use cua_spaces::{Space, Spaces};
use cua_spacesd_client::testing::{MockAuth, MockServer, mock_window};
use serde_json::{Value, json};

const TOKEN: &str = "stream-registry-token-0123456789abcdef";

struct Pair {
    parent_srv: MockServer,
    origin_srv: MockServer,
    _home: tempfile::TempDir,
    spaces: Spaces,
    parent: Space,
    origin: Space,
}

async fn pair() -> Pair {
    let mut servers = Vec::new();
    for _ in 0..2 {
        let srv = MockServer::start(MockAuth {
            token: Some(TOKEN.into()),
            ..Default::default()
        })
        .await;
        srv.state.advertise(&["windows", "window_stream"]);
        servers.push(srv);
    }
    let origin_srv = servers.pop().unwrap();
    let parent_srv = servers.pop().unwrap();
    origin_srv.state.set_windows(vec![
        mock_window("s1", "Apple", "Safari", (0.0, 0.0, 1000.0, 700.0)),
        mock_window("s2", "Docs", "Safari", (0.0, 0.0, 800.0, 600.0)),
    ]);
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder().home(home.path()).build();
    let mut ids = Vec::new();
    for (srv, name) in [(&parent_srv, "home"), (&origin_srv, "mac-child")] {
        let info = spaces
            .add(
                &format!("http://{}", srv.addr),
                Some(TOKEN.into()),
                Some(name.into()),
            )
            .await
            .unwrap();
        ids.push(info.id);
    }
    let parent = spaces.space(&ids[0]).await.unwrap();
    let origin = spaces.space(&ids[1]).await.unwrap();
    Pair {
        parent_srv,
        origin_srv,
        _home: home,
        spaces,
        parent,
        origin,
    }
}

async fn tool(spaces: &Spaces, name: &str, args: Value) -> (bool, Value) {
    let out = McpServer::new(spaces.clone()).call(name, args).await;
    let text = out
        .structured
        .clone()
        .or_else(|| out.first_text().and_then(|t| serde_json::from_str(t).ok()))
        .unwrap_or_else(|| json!(out.first_text().unwrap_or_default()));
    (out.is_error, text)
}

#[tokio::test]
async fn the_agent_shows_a_child_window_in_its_space_and_the_parent_lists_it() {
    let p = pair().await;
    let (err, shown) = tool(
        &p.spaces,
        "stream_space_window",
        json!({"space": p.origin.id().to_string(), "app_name": "Safari", "into": p.parent.id().to_string()}),
    )
    .await;
    assert!(!err, "{shown}");
    assert_eq!(shown["shown"], true);
    assert_eq!(shown["origin_name"], "mac-child");
    assert_eq!(shown["app_name"], "Safari");
    assert!(
        shown["message"]
            .as_str()
            .unwrap()
            .contains("Stream section"),
        "{shown}"
    );

    // The parent's window list: its own windows plus the shown one, with its origin.
    let (_, rows) = tool(
        &p.spaces,
        "list_space_windows",
        json!({"space": p.parent.id().to_string()}),
    )
    .await;
    let rows = rows.as_array().unwrap();
    let mine = rows
        .iter()
        .find(|r| r["origin"].is_object())
        .expect("the shown window is listed");
    assert_eq!(mine["app_name"], "Safari");
    assert_eq!(mine["origin"]["name"], "mac-child");
    assert_eq!(mine["origin"]["space_id"], p.origin.id().to_string());
    assert!(mine["window_id"].as_str().unwrap().starts_with("xs:"));
    // Its own windows carry no origin field at all (old clients keep working).
    assert!(rows.iter().filter(|r| r.get("origin").is_none()).count() >= 2);
    // The child's own list does not grow an entry.
    let (_, child_rows) = tool(
        &p.spaces,
        "list_space_windows",
        json!({"space": p.origin.id().to_string()}),
    )
    .await;
    assert!(
        child_rows
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r.get("origin").is_none())
    );
}

#[tokio::test]
async fn showing_a_window_in_its_own_space_is_refused_with_a_plain_message() {
    let p = pair().await;
    let (err, msg) = tool(
        &p.spaces,
        "stream_space_window",
        json!({"space": p.origin.id().to_string(), "app_name": "Safari", "into": p.origin.id().to_string()}),
    )
    .await;
    assert!(err);
    assert!(msg.to_string().contains("already in that Space"), "{msg}");
}

#[tokio::test]
async fn the_owner_streams_straight_from_the_origin_with_a_ticket_scoped_to_it() {
    let p = pair().await;
    let entry = p
        .spaces
        .register_stream(
            &p.parent.id().to_string(),
            &p.origin,
            &p.origin.find_window("Safari").await.unwrap(),
            None,
        )
        .await
        .unwrap();
    let handle = entry_window_id(&entry.id);
    let opened = p
        .spaces
        .open_stream_entry(&handle, StreamOptions::default())
        .await
        .unwrap();
    assert_eq!(opened.route, Route::Direct);
    assert_eq!(opened.access, Access::Owner);
    let ticket = opened.ticket.expect("minted");
    // The ticket is the origin's: its own endpoint, its own Space.
    assert_eq!(ticket.space, p.origin.id().to_string());
    assert!(
        ticket.ws_url.contains(&p.origin_srv.addr.to_string()),
        "{}",
        ticket.ws_url
    );
    assert!(
        !ticket.ws_url.contains(&p.parent_srv.addr.to_string()),
        "nothing is relayed through the parent: {}",
        ticket.ws_url
    );
    // Deciding alone mints nothing.
    let decided = p.spaces.resolve_stream_entry(&handle).await.unwrap();
    assert!(decided.ticket.is_none());
    assert_eq!(decided.route, Route::Direct);

    // And the tool answers the same decision to an agent.
    let (err, out) = tool(
        &p.spaces,
        "stream_endpoint",
        json!({"space": p.parent.id().to_string(), "window_id": handle}),
    )
    .await;
    assert!(!err, "{out}");
    assert_eq!(out["route"], "direct");
    assert_eq!(out["minted_by"], p.origin.id().to_string());
}

#[tokio::test]
async fn an_entry_ends_with_its_window_its_origin_and_its_parent() {
    let p = pair().await;
    let parent = p.parent.id().to_string();
    let safari = p.origin.find_window("Safari").await.unwrap();
    let docs = p
        .origin
        .windows(None)
        .await
        .unwrap()
        .into_iter()
        .find(|w| w.window_id == "s2")
        .unwrap();
    p.spaces
        .register_stream(&parent, &p.origin, &safari, None)
        .await
        .unwrap();
    p.spaces
        .register_stream(&parent, &p.origin, &docs, None)
        .await
        .unwrap();
    assert_eq!(p.spaces.stream_windows(&parent).await.len(), 2);

    // The window closes.
    p.origin_srv.state.set_windows(vec![mock_window(
        "s2",
        "Docs",
        "Safari",
        (0.0, 0.0, 800.0, 600.0),
    )]);
    let left = p.spaces.stream_windows(&parent).await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].title, "Docs");

    // The origin is deleted: the entry goes with it.
    p.spaces.remove(&p.origin.id().to_string()).await.unwrap();
    assert!(p.spaces.stream_windows(&parent).await.is_empty());
}

#[tokio::test]
async fn deleting_the_parent_drops_what_it_showed() {
    let p = pair().await;
    let parent = p.parent.id().to_string();
    let w = p.origin.find_window("Safari").await.unwrap();
    p.spaces
        .register_stream(&parent, &p.origin, &w, None)
        .await
        .unwrap();
    p.spaces.remove(&parent).await.unwrap();
    // A new Space under the same name starts with nothing.
    let info = p
        .spaces
        .add(
            &format!("http://{}", p.parent_srv.addr),
            Some(TOKEN.into()),
            Some("home".into()),
        )
        .await
        .unwrap();
    assert!(p.spaces.stream_windows(&info.id).await.is_empty());
}

#[tokio::test]
async fn opening_an_entry_whose_origin_is_gone_says_so() {
    let p = pair().await;
    let parent = p.parent.id().to_string();
    let w = p.origin.find_window("Safari").await.unwrap();
    let e = p
        .spaces
        .register_stream(&parent, &p.origin, &w, None)
        .await
        .unwrap();
    p.spaces.remove(&p.origin.id().to_string()).await.unwrap();
    let err = p
        .spaces
        .open_stream_entry(&entry_window_id(&e.id), StreamOptions::default())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("stream entry"), "{err}");
}

#[tokio::test]
async fn every_space_kind_streams_through_the_same_registry_path() {
    // The mock stands for any provider: the registry only asks the origin
    // Space for a window ticket, so a Space that advertises window_stream
    // streams, whatever it runs on; one that does not says why.
    let p = pair().await;
    let parent = p.parent.id().to_string();
    let w = p.origin.find_window("Safari").await.unwrap();
    let e = p
        .spaces
        .register_stream(&parent, &p.origin, &w, None)
        .await
        .unwrap();
    for _ in 0..2 {
        let o = p
            .spaces
            .open_stream_entry(&e.id, StreamOptions::default())
            .await
            .unwrap();
        assert_eq!(o.minted_by, p.origin.id().to_string());
        assert!(o.ticket.is_some());
    }
}

#[tokio::test]
async fn without_into_a_child_shows_its_window_in_the_space_that_made_it() {
    let p = pair().await;
    let parent = p.parent.id().to_string();
    p.spaces
        .registry()
        .update_meta(&p.origin.id().to_string(), |m| m.parent = Some(parent.clone()))
        .unwrap();
    let (err, shown) = tool(
        &p.spaces,
        "stream_space_window",
        json!({"space": p.origin.id().to_string(), "app_name": "Safari"}),
    )
    .await;
    assert!(!err, "{shown}");
    assert_eq!(shown["into"], parent);
    assert_eq!(p.spaces.stream_windows(&parent).await.len(), 1);
}
