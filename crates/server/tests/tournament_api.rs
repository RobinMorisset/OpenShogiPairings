//! Players, rounds, teams, boards and the request validation around them.

use axum::http::StatusCode;
use osp_server::{router, AppState};
use serde_json::json;
use uuid::Uuid;

mod common;
use common::*;

#[tokio::test]
async fn health_endpoint_returns_ok() {
    let (status, body) = send(router(AppState::default()), get("/api/health")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn get_tournament_is_404_for_an_unknown_id() {
    let (status, body) = send(router(AppState::default()), get(&t(Uuid::new_v4(), ""))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"].is_string());
}

#[tokio::test]
async fn create_then_add_and_remove_player() {
    let state = AppState::default();
    let id = create(&state, "Paris Open").await;

    let (status, body) = send(router(state.clone()), get(&t(id, ""))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tournament"]["name"], "Paris Open");
    // Named rather than spelled out: what this asserts is whatever this build
    // writes, so a format bump is not a reason for it to fail.
    assert_eq!(
        body["tournament"]["format_version"],
        osp_core::TOURNAMENT_FORMAT_VERSION
    );
    assert!(body["tournament"]["players"].as_array().unwrap().is_empty());
    assert!(body["undo_label"].is_null()); // nothing to undo on a fresh tournament

    // Register a player.
    let (status, body) = send(
        router(state.clone()),
        json_req(
            "POST",
            &t(id, "/players"),
            json!({ "last_name": "Kobayashi", "first_name": "Taichi", "rating": 2556, "nationality": "jp" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let players = body["tournament"]["players"].as_array().unwrap();
    assert_eq!(players.len(), 1);
    assert_eq!(players[0]["last_name"], "Kobayashi");
    assert_eq!(players[0]["first_name"], "Taichi");
    assert_eq!(players[0]["nationality"], "JP"); // uppercased server-side
                                                 // A mutation is now undoable, and the button can say what it would take back.
    assert_eq!(body["undo_label"]["code"], "register_player");
    assert_eq!(body["undo_label"]["values"]["name"], "Kobayashi Taichi");
    let player_id = players[0]["id"].as_str().unwrap().to_string();

    // Remove that player.
    let (status, body) = send(
        router(state.clone()),
        delete(&t(id, &format!("/players/{player_id}"))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["tournament"]["players"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn import_players_csv_registers_the_roster_as_one_undo_step() {
    let state = AppState::default();
    let id = create(&state, "Paris Open").await;

    // Reordered columns, a semicolon delimiter, and a French header — all
    // handled by the shared osp-core parser.
    let csv = "Prénom;Nom;Classement;Club\n\
               Ann;Alpha;2000;Paris\n\
               Bo;Beta;;Lyon\n";
    let (status, body) = send(
        router(state.clone()),
        post_text(&t(id, "/players/import-csv"), csv),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let players = body["tournament"]["players"].as_array().unwrap();
    assert_eq!(players.len(), 2);
    assert_eq!(players[0]["last_name"], "Alpha");
    assert_eq!(players[0]["first_name"], "Ann");
    assert_eq!(players[0]["rating"], 2000);
    assert_eq!(players[0]["club"], "Paris");
    // The whole import is a single mutation, so one undo clears all of it.
    assert_eq!(body["undo_label"]["code"], "import_players");
    let (_, body) = send(router(state.clone()), post_empty(&t(id, "/undo"))).await;
    assert!(body["tournament"]["players"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn import_players_csv_skips_players_already_registered() {
    let state = AppState::default();
    let id = create(&state, "Paris Open").await;

    let csv = "Last name,First name,ELO\nAlpha,Ann,2000\n";
    let (_, body) = send(
        router(state.clone()),
        post_text(&t(id, "/players/import-csv"), csv),
    )
    .await;
    assert_eq!(body["skipped_duplicates"].as_array().unwrap().len(), 0);

    // The same roster again, with one new player and the first one respelled:
    // only the new player is registered, and the skipped name comes back so the
    // referee is told rather than left with two Ann Alphas.
    let (status, body) = send(
        router(state.clone()),
        post_text(
            &t(id, "/players/import-csv"),
            "Last name,First name,ELO\nALPHA,ann,2000\nBeta,Bo,1800\n",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let players = body["tournament"]["players"].as_array().unwrap();
    assert_eq!(players.len(), 2);
    assert_eq!(players[0]["last_name"], "Alpha");
    assert_eq!(players[1]["last_name"], "Beta");
    assert_eq!(body["skipped_duplicates"], json!(["ALPHA ann"]));
}

#[tokio::test]
async fn import_players_csv_rejects_a_malformed_file() {
    let state = AppState::default();
    let id = create(&state, "Paris Open").await;

    // A header with no last-name/first-name column → 400, nothing imported.
    let (status, body) = send(
        router(state.clone()),
        post_text(&t(id, "/players/import-csv"), "ELO,Club\n2000,Paris\n"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("last-name"));
    // A stable machine code accompanies the message so the client localizes it.
    assert_eq!(body["code"], "csv_missing_name_columns");

    // A row with a blank last name → 400 with the offending rows for the client.
    let (status, body) = send(
        router(state.clone()),
        post_text(
            &t(id, "/players/import-csv"),
            "Last name,First name\nAlpha,Ann\n,Bo\n",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "csv_rows_missing_last_name");
    assert_eq!(body["values"]["rows"], "3"); // the second data row (file row 3)
                                             // All-or-nothing: nothing was imported.
    let (_, body) = send(router(state.clone()), get(&t(id, ""))).await;
    assert!(body["tournament"]["players"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn licence_check_reports_the_unlicensed_players_of_one_nationality() {
    let state = AppState::default();
    let id = create(&state, "Paris Open").await;

    // Two French players and a Japanese one; the licence list carries only one
    // of the French pair, and spells the other one a letter off.
    let csv = "Prénom;Nom\nAnn;Alpha\nBo;Beto\n";
    for (last, first, nat) in [
        ("Alpha", "Ann", "FR"),
        ("Beta", "Bo", "fr"), // stored uppercased, so this is the same nationality
        ("Kobayashi", "Taichi", "JP"),
    ] {
        send(
            router(state.clone()),
            json_req(
                "POST",
                &t(id, "/players"),
                json!({ "last_name": last, "first_name": first, "nationality": nat }),
            ),
        )
        .await;
    }
    let (_, body) = send(router(state.clone()), get(&t(id, ""))).await;
    let players = body["tournament"]["players"].as_array().unwrap().clone();
    let beta_id = players[1]["id"].as_str().unwrap();

    let (status, body) = send(
        router(state.clone()),
        json_req(
            "POST",
            &t(id, "/players/licence-check"),
            json!({ "nationality": "FR", "csv": csv }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["listed"], 2);
    assert_eq!(body["checked"], 2); // the Japanese player is out of scope

    // Beta is still missing — a near miss never counts as a licence — and the
    // list's own spelling rides along for the referee to judge.
    assert_eq!(
        body["missing"],
        json!([{ "id": beta_id, "near_misses": ["Beto Bo"] }])
    );

    // Read-only: the check registered nobody and left nothing to undo.
    let (_, body) = send(router(state.clone()), get(&t(id, ""))).await;
    assert_eq!(body["tournament"]["players"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn licence_check_rejects_a_blank_nationality_and_a_malformed_list() {
    let state = AppState::default();
    let id = create(&state, "Paris Open").await;

    // No nationality to check: a 400 rather than silently checking the players
    // who have none.
    let (status, body) = send(
        router(state.clone()),
        json_req(
            "POST",
            &t(id, "/players/licence-check"),
            json!({ "nationality": "  ", "csv": "Last name,First name\nAlpha,Ann\n" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("nationality"));

    // An unusable list is a 400 with the same machine code the import uses —
    // never an empty `missing` that would read as "everyone has paid".
    let (status, body) = send(
        router(state.clone()),
        json_req(
            "POST",
            &t(id, "/players/licence-check"),
            json!({ "nationality": "FR", "csv": "Licence,Club\n12345,Paris\n" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "csv_missing_name_columns");
}

#[tokio::test]
async fn add_and_remove_point_adjustment_over_http() {
    let state = AppState::default();
    let id = create(&state, "Cup").await;
    let (_, body) = send(
        router(state.clone()),
        json_req("POST", &t(id, "/players"), json!({ "last_name": "Alice" })),
    )
    .await;
    let pid = body["tournament"]["players"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Apply a bonus.
    let (status, body) = send(
        router(state.clone()),
        json_req(
            "POST",
            &t(id, &format!("/players/{pid}/adjustments")),
            json!({ "delta": 2, "reason": "Fair-play bonus" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let adjustments = body["tournament"]["players"][0]["adjustments"]
        .as_array()
        .unwrap();
    assert_eq!(adjustments.len(), 1);
    assert_eq!(adjustments[0]["delta"], 2);
    assert_eq!(adjustments[0]["reason"], "Fair-play bonus");
    let adjustment_id = adjustments[0]["id"].as_str().unwrap().to_string();

    // A blank reason is rejected.
    let (status, _) = send(
        router(state.clone()),
        json_req(
            "POST",
            &t(id, &format!("/players/{pid}/adjustments")),
            json!({ "delta": 1, "reason": "  " }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Remove the bonus.
    let (status, body) = send(
        router(state.clone()),
        delete(&t(
            id,
            &format!("/players/{pid}/adjustments/{adjustment_id}"),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Empty adjustments are omitted from the JSON entirely (skip_serializing_if).
    assert!(body["tournament"]["players"][0]["adjustments"].is_null());

    // Removing it again is a 404.
    let (status, _) = send(
        router(state.clone()),
        delete(&t(
            id,
            &format!("/players/{pid}/adjustments/{adjustment_id}"),
        )),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn edit_then_undo_reverts_step_by_step() {
    let state = AppState::default();
    let id = create(&state, "Cup").await;
    let (_, body) = send(
        router(state.clone()),
        json_req(
            "POST",
            &t(id, "/players"),
            json!({ "last_name": "Alice", "rating": 1500 }),
        ),
    )
    .await;
    let pid = body["tournament"]["players"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Edit the rating.
    let (status, body) = send(
        router(state.clone()),
        json_req(
            "PUT",
            &t(id, &format!("/players/{pid}")),
            json!({ "last_name": "Alice", "rating": 1900 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tournament"]["players"][0]["rating"], 1900);

    // Undo the edit → rating restored.
    let (status, body) = send(router(state.clone()), post_empty(&t(id, "/undo"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tournament"]["players"][0]["rating"], 1500);
    // The add below it is still undoable, and now names itself.
    assert_eq!(body["undo_label"]["code"], "register_player");
    assert_eq!(body["undo_label"]["values"]["name"], "Alice");

    // Undo the add → back to empty, nothing left to undo.
    let (status, body) = send(router(state.clone()), post_empty(&t(id, "/undo"))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["tournament"]["players"].as_array().unwrap().is_empty());
    assert!(body["undo_label"].is_null());

    // One undo too many is refused, not silently answered `200` with an
    // unchanged tournament — which a client would read as an undo that landed.
    let (status, body) = send(router(state.clone()), post_empty(&t(id, "/undo"))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("nothing left to undo"),
        "{body}"
    );
}

#[tokio::test]
async fn start_round_pairs_current_players() {
    let state = AppState::default();
    let id = create(&state, "Cup").await;
    for name in ["Alice", "Bob", "Carol"] {
        send(
            router(state.clone()),
            json_req("POST", &t(id, "/players"), json!({ "last_name": name })),
        )
        .await;
    }
    send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;

    let (status, body) = send(router(state.clone()), post_empty(&t(id, "/rounds"))).await;
    assert_eq!(status, StatusCode::CREATED);
    let rounds = body["tournament"]["rounds"].as_array().unwrap();
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0]["number"], 1);
    assert_eq!(rounds[0]["boards"].as_array().unwrap().len(), 1); // 3 → 1 board

    // + a bye, serialized as the round's one sit-out.
    let sitouts = rounds[0]["sitouts"].as_array().unwrap();
    assert_eq!(sitouts.len(), 1);
    assert!(sitouts[0]["player"].is_number());
    assert_eq!(sitouts[0]["kind"], "bye");
    assert_eq!(sitouts[0]["value"], "full");
}

#[tokio::test]
async fn round_lifecycle_is_gated_over_http() {
    let state = AppState::default();
    let id = create(&state, "Cup").await;
    for name in ["Alice", "Bob"] {
        send(
            router(state.clone()),
            json_req("POST", &t(id, "/players"), json!({ "last_name": name })),
        )
        .await;
    }

    // Preparing round 1 auto-finalizes registration in the same step, then
    // confirm round 1.
    let (status, body) = send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tournament"]["registration_finalized"], true);
    let (status, _) = send(router(state.clone()), post_empty(&t(id, "/rounds"))).await;
    assert_eq!(status, StatusCode::CREATED);

    // Round 1 isn't complete until its game is played, so round 2 can't
    // start yet.
    let (status, _) = send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Recording the game's result completes the round automatically.
    let (status, body) = send(
        router(state.clone()),
        json_req(
            "POST",
            &t(id, "/rounds/1/boards/0/result"),
            json!({ "clicked": "player1" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tournament"]["rounds"][0]["completed"], true);

    // Now round 2 can be prepared and started.
    send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;
    let (status, _) = send(router(state.clone()), post_empty(&t(id, "/rounds"))).await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn preparing_round_one_finalizes_as_a_single_undo_step() {
    let state = AppState::default();
    let id = create(&state, "Cup").await;
    for name in ["Alice", "Bob"] {
        send(
            router(state.clone()),
            json_req("POST", &t(id, "/players"), json!({ "last_name": name })),
        )
        .await;
    }

    // Preparing round 1 both finalizes registration and opens the draft.
    let (status, body) = send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tournament"]["registration_finalized"], true);
    assert!(body["tournament"]["draft"].is_object());

    // A single undo reverts *both*: back to open registration, no draft.
    // (Were it two steps, one undo would leave registration still finalized.)
    let (status, body) = send(router(state.clone()), post_empty(&t(id, "/undo"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tournament"]["registration_finalized"], false);
    assert!(body["tournament"]["draft"].is_null());
}

/// The team roster routes, end to end: a team tournament is configured,
/// rostered, finalized and paired entirely over HTTP.
#[tokio::test]
async fn team_rosters_are_built_and_played_over_http() {
    let state = AppState::default();
    let id = create(&state, "Teams").await;

    // Switch to team mode with two players per team.
    let (status, _) = send(
        router(state.clone()),
        json_req(
            "PUT",
            &t(id, "/settings"),
            json!({ "teams": { "size": 2 } }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Four players.
    let mut players = Vec::new();
    for name in ["Alpha", "Beta", "Gamma", "Delta"] {
        let (_, body) = send(
            router(state.clone()),
            json_req(
                "POST",
                &t(id, "/players"),
                json!({ "last_name": name, "rating": 1500 }),
            ),
        )
        .await;
        let list = body["tournament"]["players"].as_array().unwrap();
        players.push(list.last().unwrap()["id"].as_str().unwrap().to_string());
    }

    // Two teams, two members each.
    let mut teams = Vec::new();
    for name in ["East", "West"] {
        let (status, body) = send(
            router(state.clone()),
            json_req("POST", &t(id, "/teams"), json!({ "name": name })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let list = body["tournament"]["teams"].as_array().unwrap();
        teams.push(list.last().unwrap()["id"].as_str().unwrap().to_string());
    }
    for (i, player) in players.iter().enumerate() {
        let (status, _) = send(
            router(state.clone()),
            json_req(
                "POST",
                &t(id, &format!("/teams/{}/members", teams[i / 2])),
                json!({ "player_id": player }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // A duplicate name is refused, with the code the client localizes.
    let (status, body) = send(
        router(state.clone()),
        json_req("POST", &t(id, "/teams"), json!({ "name": " east " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "duplicate_team_name");

    // Reorder one team's boards, then reset the order by rating.
    let (status, body) = send(
        router(state.clone()),
        json_req(
            "PUT",
            &t(id, &format!("/teams/{}/board-order", teams[0])),
            json!({ "order": [players[1], players[0]] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["tournament"]["teams"][0]["members"][0],
        json!(players[1])
    );
    send(
        router(state.clone()),
        post_empty(&t(id, &format!("/teams/{}/sort-by-rating", teams[0]))),
    )
    .await;

    // Pair round 1: one match, two boards, and the team standings appear.
    start_round(&state, id).await;
    let (status, body) = send(router(state.clone()), get(&t(id, ""))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["tournament"]["rounds"][0]["boards"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let table = body["team_standings"].as_array().unwrap();
    assert_eq!(table.len(), 2);
    assert!(table[0]["members"].as_array().unwrap().len() == 2);

    // The round's boards arrive already grouped into the match they belong to,
    // so no client re-derives that grouping. Nothing is decided yet.
    let matches = body["team_matches"][0].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["boards"], json!([0, 1]));
    assert_eq!(matches[0]["decided"], false);
    assert_eq!(
        (&matches[0]["wins1"], &matches[0]["wins2"]),
        (&json!(0), &json!(0))
    );

    // Recording a board moves the match score with it — in half-wins, like
    // every `HalfWins` on the wire, so one board win is 2.
    let (status, body) = send(
        router(state.clone()),
        json_req(
            "POST",
            &t(id, "/rounds/1/boards/0/result"),
            json!({ "clicked": "player1" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let played = &body["team_matches"][0][0];
    assert_eq!((&played["wins1"], &played["wins2"]), (&json!(2), &json!(0)));
    assert_eq!(played["decided"], false); // board 1 is still to play

    // A finalized team tournament takes no late registration.
    let (status, body) = send(
        router(state.clone()),
        json_req("POST", &t(id, "/players"), json!({ "last_name": "Late" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "no_late_registration_in_team_mode");
}

/// The draft route dispatches on the tournament's mode: a team round takes
/// forced *matches*, and the player-level lists are refused rather than
/// quietly dropped.
#[tokio::test]
async fn a_team_draft_takes_forced_matches_and_refuses_forced_boards() {
    let state = AppState::default();
    let id = create(&state, "Teams").await;
    send(
        router(state.clone()),
        json_req(
            "PUT",
            &t(id, "/settings"),
            json!({ "teams": { "size": 2 } }),
        ),
    )
    .await;
    let mut players = Vec::new();
    for i in 0..8 {
        let (_, body) = send(
            router(state.clone()),
            json_req(
                "POST",
                &t(id, "/players"),
                json!({ "last_name": format!("P{i}"), "rating": 2000 - i * 10 }),
            ),
        )
        .await;
        let list = body["tournament"]["players"].as_array().unwrap();
        players.push(list.last().unwrap()["id"].as_str().unwrap().to_string());
    }
    for k in 0..4 {
        let (_, body) = send(
            router(state.clone()),
            json_req("POST", &t(id, "/teams"), json!({ "name": format!("T{k}") })),
        )
        .await;
        let team = body["tournament"]["teams"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        for player in &players[k * 2..k * 2 + 2] {
            send(
                router(state.clone()),
                json_req(
                    "POST",
                    &t(id, &format!("/teams/{team}/members")),
                    json!({ "player_id": player }),
                ),
            )
            .await;
        }
    }
    send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;

    // A player-level forced board is refused, naming why.
    let (status, body) = send(
        router(state.clone()),
        json_req(
            "PUT",
            &t(id, "/draft"),
            json!({ "forced_boards": [{ "player1": 1, "player2": 3 }] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("paired by team"));

    // A forced *match* is taken, and honoured when the round is confirmed.
    let (status, body) = send(
        router(state.clone()),
        json_req(
            "PUT",
            &t(id, "/draft"),
            json!({ "forced_matches": [{ "team1": 1, "team2": 4 }] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["tournament"]["draft"]["forced_matches"],
        json!([{ "team1": 1, "team2": 4 }])
    );
    send(router(state.clone()), post_empty(&t(id, "/rounds"))).await;

    let (_, body) = send(router(state.clone()), get(&t(id, ""))).await;
    let boards = body["tournament"]["rounds"][0]["boards"]
        .as_array()
        .unwrap();
    // Teams 1 and 4 are board 1 and board 2 of the forced match, so their
    // two boards are the ones tagged `forced`.
    let forced: Vec<&serde_json::Value> = boards
        .iter()
        .filter(|b| b["source"]["kind"] == "forced")
        .collect();
    assert_eq!(forced.len(), 2, "{boards:#?}");
}

/// An individual tournament carries no team table at all, rather than an
/// empty one that would read as "no teams yet".
#[tokio::test]
async fn an_individual_tournament_has_no_team_standings() {
    let state = AppState::default();
    let id = create(&state, "Solo").await;
    let (_, body) = send(router(state.clone()), get(&t(id, ""))).await;
    assert!(body.get("team_standings").is_none());
}

#[tokio::test]
async fn set_board_result_toggles_winner() {
    let state = AppState::default();
    let id = create(&state, "Cup").await;
    for name in ["Alice", "Bob"] {
        send(
            router(state.clone()),
            json_req("POST", &t(id, "/players"), json!({ "last_name": name })),
        )
        .await;
    }
    start_round(&state, id).await;

    // Click player 1 → they win.
    let (status, body) = send(
        router(state.clone()),
        json_req(
            "POST",
            &t(id, "/rounds/1/boards/0/result"),
            json!({ "clicked": "player1" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["tournament"]["rounds"][0]["boards"][0]["record"],
        json!({ "kind": "short", "outcome": { "kind": "won", "winner": "player1" } })
    );

    // Bad board index → 404.
    let (status, _) = send(
        router(state.clone()),
        json_req(
            "POST",
            &t(id, "/rounds/1/boards/9/result"),
            json!({ "clicked": "player1" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A misspelled body field must be rejected, not ignored: silently dropping
/// a typo'd `cup_size` would finalize round 1 with no cup bracket at all,
/// and the referee would only notice rounds later.
#[tokio::test]
async fn prepare_round_rejects_an_unknown_body_field() {
    let state = AppState::default();
    let id = create(&state, "Cup").await;
    for name in ["Alice", "Bob"] {
        send(
            router(state.clone()),
            json_req("POST", &t(id, "/players"), json!({ "last_name": name })),
        )
        .await;
    }
    // Axum's own body rejection, so the response is plain text, not our
    // JSON error envelope.
    let (status, body) = send_text(
        router(state.clone()),
        json_req("POST", &t(id, "/rounds/prepare"), json!({ "cupsize": 8 })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("cupsize"), "unhelpful rejection: {body}");

    // ...and nothing was mutated by the rejected call.
    let (_, body) = send(router(state.clone()), get(&t(id, ""))).await;
    assert_eq!(body["tournament"]["registration_finalized"], false);
    assert!(body["tournament"]["draft"].is_null());
}

#[tokio::test]
async fn start_round_needs_two_players_is_400() {
    let state = AppState::default();
    let id = create(&state, "Cup").await;
    send(
        router(state.clone()),
        json_req("POST", &t(id, "/players"), json!({ "last_name": "Solo" })),
    )
    .await;
    send(router(state.clone()), post_empty(&t(id, "/rounds/prepare"))).await;
    let (status, _) = send(router(state.clone()), post_empty(&t(id, "/rounds"))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn edit_missing_player_is_404() {
    let state = AppState::default();
    let id = create(&state, "Cup").await;
    let (status, _) = send(
        router(state.clone()),
        json_req(
            "PUT",
            &t(id, &format!("/players/{}", Uuid::new_v4())),
            json!({ "last_name": "Ghost" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn add_player_to_an_unknown_tournament_is_404() {
    let (status, _) = send(
        router(AppState::default()),
        json_req(
            "POST",
            &t(Uuid::new_v4(), "/players"),
            json!({ "last_name": "Alice" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn blank_player_name_is_400() {
    let state = AppState::default();
    let id = create(&state, "Paris Open").await;
    let (status, _) = send(
        router(state.clone()),
        json_req("POST", &t(id, "/players"), json!({ "last_name": "   " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// The bulk eligibility endpoint is all-or-nothing.
///
/// The point of it existing: the client used to loop over the single-player
/// endpoint, so a failure partway through left some players changed and some
/// not. Here a list containing one unknown id must change nobody — including the
/// players named *before* the bad one, which is what a loop would already have
/// written.
#[tokio::test]
async fn bulk_eligibility_applies_to_everyone_or_to_nobody() {
    let state = AppState::default();
    let id = create(&state, "Cup").await;

    let mut ids = Vec::new();
    for name in ["Alice", "Bob", "Carol"] {
        let (_, body) = send(
            router(state.clone()),
            json_req("POST", &t(id, "/players"), json!({ "last_name": name })),
        )
        .await;
        let players = body["tournament"]["players"].as_array().unwrap();
        ids.push(players.last().unwrap()["id"].as_str().unwrap().to_string());
    }

    // `eligible` defaults to false and is skipped when false, so an untouched
    // player carries no such key at all — which is exactly the state the failed
    // call below must leave the untouched ones in.
    let eligible = |p: &serde_json::Value| p["eligible"] == json!(true);

    let (status, body) = send(
        router(state.clone()),
        json_req(
            "POST",
            &t(id, "/players/eligible"),
            json!({ "player_ids": ids, "eligible": true }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let players = body["tournament"]["players"].as_array().unwrap();
    assert!(
        players.iter().all(eligible),
        "every named player should have been set: {players:#?}"
    );
    let version_after_bulk = body["version"].clone();

    // The same call with an unknown id in the *middle*, asking for the opposite
    // flag. Alice comes before it, so a loop would already have cleared her.
    let with_unknown = vec![ids[0].clone(), Uuid::new_v4().to_string(), ids[1].clone()];
    let (status, _) = send(
        router(state.clone()),
        json_req(
            "POST",
            &t(id, "/players/eligible"),
            json!({ "player_ids": with_unknown, "eligible": false }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Nobody moved, and the failed call did not even bump the version.
    let (_, body) = send(router(state.clone()), get(&t(id, ""))).await;
    let players = body["tournament"]["players"].as_array().unwrap();
    assert!(
        players.iter().all(eligible),
        "a rejected bulk must leave every player as they were: {players:#?}"
    );
    assert_eq!(
        body["version"], version_after_bulk,
        "a rejected mutation should not bump the version"
    );
}

/// Every path this crate registers has a row in `docs/reference/api.md`.
///
/// The ts-rs bindings have a gate that fails the build when a Rust type moves
/// without them. This table has nothing of the sort, because it is prose: a
/// route added and not written up is invisible until somebody goes looking for
/// an endpoint that was never documented. `POST /players/eligible` shipped that
/// way and was caught by a reader asking, which is not a process.
///
/// All four routing modules, not just the per-tournament one: the registry
/// routes in `registry.rs` and `lib.rs`, and the reader routes in `public.rs`,
/// are in the same table and can go stale the same way. Nesting is not
/// flattened, because the table does not flatten it either — `tournament.rs` and
/// `public.rs` are written relative to `/api/tournaments/{id}` on both sides.
///
/// Paths only, with placeholder *names* erased first — `{player_id}` in the
/// router against `{id}` in the table, `{round_number}` against `{n}`. The doc
/// names its parameters for legibility and that is not drift worth failing a
/// build over; a missing path is.
///
/// One direction only, deliberately: `.fallback` serves the built frontend and
/// has no business in an API table, so "documented but not registered" is not
/// checked here.
#[test]
fn every_route_is_documented_in_the_api_doc() {
    const API_DOC: &str = include_str!("../../../docs/reference/api.md");
    /// Every module that registers routes. Checked against the source directory
    /// below, so adding a fifth cannot quietly go unexamined.
    const ROUTING_MODULES: [(&str, &str); 4] = [
        ("lib.rs", include_str!("../src/lib.rs")),
        ("registry.rs", include_str!("../src/registry.rs")),
        ("public.rs", include_str!("../src/public.rs")),
        ("tournament.rs", include_str!("../src/tournament.rs")),
    ];

    /// `/players/{player_id}/eligible` -> `/players/{}/eligible`.
    fn erase_placeholder_names(path: &str) -> String {
        let mut out = String::new();
        let mut depth = 0u32;
        for c in path.chars() {
            match c {
                '{' => {
                    depth += 1;
                    if depth == 1 {
                        out.push_str("{}");
                    }
                }
                '}' => depth = depth.saturating_sub(1),
                _ if depth == 0 => out.push(c),
                _ => {}
            }
        }
        out
    }

    // Built rather than written out, so that this needle does not match the line
    // it is written on — the modules above are scanned as text, and one of them
    // could one day be this file's neighbour in a merge.
    let call = concat!(".", "route", "(");

    // What the list above claims is complete, checked against the directory. A
    // new routing module would otherwise be exempt from this test by the simple
    // fact of nobody having added it here.
    let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut unlisted: Vec<String> = std::fs::read_dir(&src_dir)
        .expect("crates/server/src should be readable")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_str()?.to_owned();
            let text = std::fs::read_to_string(&path).ok()?;
            // Routes behind `#[cfg(test)]` are fixtures (error.rs builds a
            // `/boom` router to check the error body), not API.
            let production = text.split("#[cfg(test)]").next().unwrap_or("").to_owned();
            (production.contains(call) && !ROUTING_MODULES.iter().any(|(m, _)| *m == name))
                .then_some(name)
        })
        .collect();
    unlisted.sort();
    assert!(
        unlisted.is_empty(),
        "these modules register routes but are not in ROUTING_MODULES, so this test \
         never looked at them: {unlisted:?}"
    );

    let registered: Vec<(&str, String)> = ROUTING_MODULES
        .iter()
        .flat_map(|(module, text)| {
            text.split("#[cfg(test)]")
                .next()
                .unwrap_or("")
                .split(call)
                .skip(1)
                .filter_map(move |chunk| {
                    let rest = chunk.split_once('"')?.1;
                    let path = rest.split_once('"')?.0;
                    path.starts_with('/')
                        .then(|| (*module, erase_placeholder_names(path)))
                })
                .collect::<Vec<_>>()
        })
        .collect();

    // A silent extraction failure must not read as "everything is documented".
    assert!(
        registered.len() > 40,
        "found only {} routes across {} modules — the extractor is broken, not the docs",
        registered.len(),
        ROUTING_MODULES.len()
    );

    let documented: std::collections::HashSet<String> = API_DOC
        .lines()
        .filter_map(|line| {
            let cell = line.trim_start().strip_prefix("| `")?;
            let cell = cell.split_once('`')?.0;
            let path = cell.split_once(' ')?.1.trim();
            let path = path.split('?').next().unwrap_or(path);
            Some(erase_placeholder_names(path))
        })
        .collect();

    let mut missing: Vec<String> = registered
        .iter()
        .filter(|(_, path)| !documented.contains(path))
        .map(|(module, path)| format!("    {path}   ({module})"))
        .collect();
    missing.sort();
    missing.dedup();

    assert!(
        missing.is_empty(),
        "these routes are registered in crates/server/src but have no row in \
         docs/reference/api.md:\n{}",
        missing.join("\n")
    );
}
