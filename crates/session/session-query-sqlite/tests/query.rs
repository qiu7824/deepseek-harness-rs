//! Rust port of `packages/session-query/session-query-sqlite/tests/query.spec.ts`:
//! request normalization, predicate compilation, query identity, and snippet
//! presentation.

use dsh_session::session_id;
use dsh_session_query::{
    SessionAvailability, SessionEventResultFilter, SessionEventSearchRequest, SessionEventSurface,
    SessionResultFilter, SessionQueryErrorCode, SessionSearchRequest, session_search_cursor,
};
use dsh_session_query_sqlite::Config;
use dsh_session_query_sqlite::query::{
    Binding, RequestFingerprint, SQLITE_FTS5_OUTER_PREDICATE_LIMIT, SQLITE_MAX_PAGE_LIMIT,
    build_event_where, build_session_where, make_snippet, normalize_event_request,
    normalize_session_request, quote_fts_data, request_fingerprint, FTS_HIGHLIGHT_END,
    FTS_HIGHLIGHT_START,
};

fn limits() -> Config {
    Config {
        path: ":memory:".to_string(),
        default_limit: Some(2),
        max_limit: Some(3),
        ..Default::default()
    }
}

fn code(error: &dsh_session_query::SessionQueryError) -> SessionQueryErrorCode {
    error.code
}

#[test]
fn normalizes_both_scopes_defaults_arrays_and_limits() {
    let resolved = dsh_session_query_sqlite::resolve_config(&limits()).expect("config");
    let sessions = normalize_session_request(
        &SessionSearchRequest {
            query: "  alpha\n beta  ".to_string(),
            ..Default::default()
        },
        &resolved,
    )
    .expect("sessions");
    assert_eq!(sessions.query, "alpha beta");
    assert!(sessions.session_filters.is_empty());
    assert!(sessions.event_filters.is_empty());
    assert_eq!(sessions.limit, 2);

    let sessions = normalize_session_request(
        &SessionSearchRequest {
            query: "needle".to_string(),
            session_filters: Some(vec![SessionResultFilter::Availability {
                values: vec![SessionAvailability::Live],
            }]),
            event_filters: Some(vec![SessionEventResultFilter::Surface {
                values: vec![SessionEventSurface::Current],
            }]),
            limit: Some(3),
            cursor: Some(session_search_cursor("next")),
        },
        &resolved,
    )
    .expect("sessions with filters");
    assert_eq!(sessions.query, "needle");
    assert_eq!(sessions.limit, 3);
    assert_eq!(sessions.cursor.as_ref().expect("cursor").as_str(), "next");

    let events = normalize_event_request(
        &SessionEventSearchRequest {
            session_id: Some(session_id("s")),
            query: "needle".to_string(),
            ..Default::default()
        },
        &resolved,
    )
    .expect("events");
    assert_eq!(events.session_id, session_id("s"));
    assert!(events.filters.is_empty());
    assert_eq!(events.limit, 2);

    let events = normalize_event_request(
        &SessionEventSearchRequest {
            session_id: Some(session_id("s")),
            query: "needle".to_string(),
            filters: Some(vec![SessionEventResultFilter::Seq {
                from: Some(1.0),
                to: None,
            }]),
            cursor: Some(session_search_cursor("next")),
            limit: None,
        },
        &resolved,
    )
    .expect("events with cursor");
    assert_eq!(events.limit, 2);
    assert_eq!(events.cursor.as_ref().expect("cursor").as_str(), "next");
}

#[test]
fn rejects_blank_nul_and_malformed_requests() {
    let resolved = dsh_session_query_sqlite::resolve_config(&limits()).expect("config");
    let error = normalize_session_request(
        &SessionSearchRequest {
            query: " \n ".to_string(),
            ..Default::default()
        },
        &resolved,
    )
    .err()
    .expect("blank query rejected");
    assert_eq!(code(&error), SessionQueryErrorCode::SessionQueryInvalidQuery);

    let error = normalize_session_request(
        &SessionSearchRequest {
            query: "bad\0query".to_string(),
            ..Default::default()
        },
        &resolved,
    )
    .err()
    .expect("NUL query rejected");
    assert_eq!(code(&error), SessionQueryErrorCode::SessionQueryInvalidQuery);

    let error = normalize_event_request(
        &SessionEventSearchRequest {
            session_id: None,
            query: "x".to_string(),
            ..Default::default()
        },
        &resolved,
    )
    .err()
    .expect("missing session id rejected");
    assert_eq!(code(&error), SessionQueryErrorCode::SessionQueryInvalidFilter);

    let error = normalize_event_request(
        &SessionEventSearchRequest {
            session_id: Some(session_id("s")),
            query: "x".to_string(),
            filters: Some(vec![SessionEventResultFilter::Seq {
                from: Some(2.0),
                to: Some(1.0),
            }]),
            ..Default::default()
        },
        &resolved,
    )
    .err()
    .expect("inverted range rejected");
    assert_eq!(code(&error), SessionQueryErrorCode::SessionQueryInvalidFilter);

    let error = normalize_session_request(
        &SessionSearchRequest {
            query: "x".to_string(),
            event_filters: Some(vec![SessionEventResultFilter::Text {
                text: "x".to_string(),
            }]),
            ..Default::default()
        },
        &resolved,
    )
    .err()
    .expect("text metadata clause rejected");
    assert_eq!(code(&error), SessionQueryErrorCode::SessionQueryInvalidFilter);

    for limit in [0, 4] {
        let error = normalize_event_request(
            &SessionEventSearchRequest {
                session_id: Some(session_id("s")),
                query: "x".to_string(),
                limit: Some(limit),
                ..Default::default()
            },
            &resolved,
        )
        .err()
        .expect("bad limit rejected");
        assert_eq!(code(&error), SessionQueryErrorCode::SessionQueryInvalidLimit);
    }

    let error = normalize_event_request(
        &SessionEventSearchRequest {
            session_id: Some(session_id("s")),
            query: "x".to_string(),
            limit: Some(SQLITE_MAX_PAGE_LIMIT + 1),
            ..Default::default()
        },
        &dsh_session_query_sqlite::ResolvedConfig {
            path: ":memory:".to_string(),
            open_at: dsh_session_query_sqlite::OpenAt::Startup,
            journal_mode: dsh_session_query_sqlite::JournalMode::Wal,
            default_limit: 1,
            max_limit: SQLITE_MAX_PAGE_LIMIT + 1,
            snippet_chars: 240,
            read_window_max: 50,
            persisted_inspect_concurrency: 4,
        },
    )
    .err()
    .expect("oversized limit rejected");
    assert_eq!(code(&error), SessionQueryErrorCode::SessionQueryInvalidLimit);
}

#[test]
fn compiles_all_session_clauses_including_empty_and_nullable() {
    let empty = build_session_where(&[]).expect("empty");
    assert_eq!(empty.sql, "");
    assert!(empty.params.is_empty());
    assert_eq!(empty.predicate_count, 0);

    let empty_values = build_session_where(&[SessionResultFilter::Id { values: vec![] }])
        .expect("empty id values");
    assert_eq!(empty_values.sql, "0");
    assert!(empty_values.params.is_empty());
    assert_eq!(empty_values.predicate_count, 1);

    let ids = build_session_where(&[SessionResultFilter::Id {
        values: vec![session_id("a"), session_id("b")],
    }])
    .expect("ids");
    assert_eq!(ids.sql, "session_id IN (?, ?)");
    assert_eq!(
        ids.params,
        vec![Binding::Text("a".into()), Binding::Text("b".into())]
    );

    let null_cwd = build_session_where(&[SessionResultFilter::Cwd {
        values: vec![None],
    }])
    .expect("null cwd");
    assert_eq!(null_cwd.sql, "(cwd IS NULL)");

    let cwd = build_session_where(&[SessionResultFilter::Cwd {
        values: vec![Some("/a".to_string())],
    }])
    .expect("cwd");
    assert_eq!(cwd.sql, "(cwd IN (?))");
    assert_eq!(cwd.params, vec![Binding::Text("/a".into())]);

    let parent = build_session_where(&[SessionResultFilter::Parent {
        values: vec![Some(session_id("p")), None],
    }])
    .expect("parent");
    assert_eq!(parent.sql, "(parent_session IN (?) OR parent_session IS NULL)");

    let combined = build_session_where(&[
        SessionResultFilter::CreatedAt {
            from: Some(1.0),
            to: Some(2.0),
        },
        SessionResultFilter::Availability { values: vec![] },
        SessionResultFilter::Availability {
            values: vec![SessionAvailability::Live, SessionAvailability::Live],
        },
        SessionResultFilter::Availability {
            values: vec![SessionAvailability::Live, SessionAvailability::Persisted],
        },
    ])
    .expect("combined");
    assert_eq!(
        combined.sql,
        "CAST(created_at AS INTEGER) >= ? AND CAST(created_at AS INTEGER) <= ? AND 0 AND live = 1"
    );
    assert_eq!(combined.predicate_count, 4);

    let bare_range = build_session_where(&[SessionResultFilter::CreatedAt {
        from: None,
        to: None,
    }])
    .expect("bare range");
    assert_eq!(bare_range.sql, "");
    assert_eq!(bare_range.predicate_count, 0);
}

#[test]
fn compiles_every_event_clause_and_empty_lists() {
    let combined = build_event_where(&[
        SessionEventResultFilter::Seq {
            from: Some(1.0),
            to: None,
        },
        SessionEventResultFilter::Time {
            from: None,
            to: Some(9.0),
        },
        SessionEventResultFilter::Type {
            values: vec!["user/message".to_string()],
        },
        SessionEventResultFilter::Surface {
            values: vec![SessionEventSurface::Current, SessionEventSurface::LogOnly],
        },
    ])
    .expect("combined");
    assert_eq!(
        combined.sql,
        "CAST(seq AS INTEGER) >= ? AND CAST(time AS INTEGER) <= ? AND type IN (?) AND surface IN (?, ?)"
    );
    assert_eq!(combined.params.len(), 5);
    assert_eq!(combined.predicate_count, 4);

    let empty = build_event_where(&[
        SessionEventResultFilter::Type { values: vec![] },
        SessionEventResultFilter::Surface { values: vec![] },
    ])
    .expect("empty lists");
    assert_eq!(empty.sql, "0 AND 0");
    assert_eq!(empty.predicate_count, 2);
}

#[test]
fn rejects_predicate_builders_above_the_fts5_outer_budget() {
    let filters: Vec<SessionResultFilter> = (0..SQLITE_FTS5_OUTER_PREDICATE_LIMIT)
        .map(|_| SessionResultFilter::Id {
            values: vec![session_id("safe")],
        })
        .collect();
    assert_eq!(
        build_session_where(&filters).expect("boundary").predicate_count,
        SQLITE_FTS5_OUTER_PREDICATE_LIMIT
    );
    let mut over = filters;
    over.push(SessionResultFilter::Id {
        values: vec![session_id("over")],
    });
    let error = build_session_where(&over).err().expect("rejected");
    assert_eq!(code(&error), SessionQueryErrorCode::SessionQueryInvalidFilter);

    let event_filters: Vec<SessionEventResultFilter> =
        (0..SQLITE_FTS5_OUTER_PREDICATE_LIMIT + 1)
            .map(|_| SessionEventResultFilter::Type {
                values: vec!["user/message".to_string()],
            })
            .collect();
    let error = build_event_where(&event_filters).err().expect("rejected");
    assert_eq!(code(&error), SessionQueryErrorCode::SessionQueryInvalidFilter);
}

#[test]
fn quotes_all_caller_match_syntax_as_data() {
    assert_eq!(quote_fts_data("say \"needle\" OR *"), "\"say \"\"needle\"\" OR *\"");
}

#[test]
fn canonicalizes_request_and_filter_ordering_in_both_scopes() {
    let session_a = request_fingerprint(&RequestFingerprint::Sessions {
        query: "needle",
        session_filters: &[
            SessionResultFilter::Cwd {
                values: vec![Some("/b".to_string()), Some("/a".to_string())],
            },
            SessionResultFilter::Parent {
                values: vec![None, Some(session_id("p"))],
            },
            SessionResultFilter::Id {
                values: vec![session_id("same"), session_id("same")],
            },
            SessionResultFilter::CreatedAt {
                from: Some(1.0),
                to: None,
            },
        ],
        event_filters: &[SessionEventResultFilter::Time {
            from: None,
            to: Some(9.0),
        }],
        limit: 2,
    });
    let session_b = request_fingerprint(&RequestFingerprint::Sessions {
        query: "needle",
        session_filters: &[
            SessionResultFilter::CreatedAt {
                from: Some(1.0),
                to: None,
            },
            SessionResultFilter::Id {
                values: vec![session_id("same"), session_id("same")],
            },
            SessionResultFilter::Parent {
                values: vec![Some(session_id("p")), None],
            },
            SessionResultFilter::Cwd {
                values: vec![Some("/a".to_string()), Some("/b".to_string())],
            },
        ],
        event_filters: &[SessionEventResultFilter::Time {
            from: None,
            to: Some(9.0),
        }],
        limit: 2,
    });
    assert_eq!(session_a, session_b);

    let event_a = request_fingerprint(&RequestFingerprint::Events {
        session_id: &session_id("s"),
        query: "needle",
        filters: &[
            SessionEventResultFilter::Seq {
                from: None,
                to: None,
            },
            SessionEventResultFilter::Surface {
                values: vec![SessionEventSurface::Shadowed, SessionEventSurface::Current],
            },
        ],
        limit: 2,
    });
    let event_b = request_fingerprint(&RequestFingerprint::Events {
        session_id: &session_id("s"),
        query: "needle",
        filters: &[
            SessionEventResultFilter::Surface {
                values: vec![SessionEventSurface::Current, SessionEventSurface::Shadowed],
            },
            SessionEventResultFilter::Seq {
                from: None,
                to: None,
            },
        ],
        limit: 2,
    });
    assert_eq!(event_a, event_b);
    let other = request_fingerprint(&RequestFingerprint::Events {
        session_id: &session_id("other"),
        query: "needle",
        filters: &[],
        limit: 2,
    });
    assert_ne!(event_a, other);
}

#[test]
fn normalizes_bounds_and_positions_snippets_by_unicode_code_point() {
    assert_eq!(make_snippet("  short\ntext  ", 20), "short text");
    assert_eq!(
        make_snippet(&format!("abcde{FTS_HIGHLIGHT_START}f{FTS_HIGHLIGHT_END}"), 1),
        "…"
    );
    assert_eq!(make_snippet("abcdefghij", 5), "abcd…");
    assert_eq!(
        make_snippet(&format!("ab{FTS_HIGHLIGHT_START}c{FTS_HIGHLIGHT_END}defghij"), 5),
        "…bcd…"
    );
    assert_eq!(
        make_snippet(&format!("ab{FTS_HIGHLIGHT_START}c{FTS_HIGHLIGHT_END}defghij"), 3),
        "…c…"
    );
    assert_eq!(
        make_snippet(&format!("abcde{FTS_HIGHLIGHT_START}f{FTS_HIGHLIGHT_END}"), 2),
        "…f"
    );
    assert_eq!(
        make_snippet(&format!("abcde{FTS_HIGHLIGHT_START}f{FTS_HIGHLIGHT_END}"), 5),
        "…cdef"
    );
    assert_eq!(
        make_snippet(
            &format!("  x—{FTS_HIGHLIGHT_START}café{FTS_HIGHLIGHT_END}\n y  "),
            20
        ),
        "x—café y"
    );
}
