use axum::{
    extract::Query,
    http::{header, StatusCode},
    response::{Html, IntoResponse, Json},
    routing::get,
    Router,
};
use serde::Deserialize;
use serde_json::json;

use crate::db::queries;
use crate::db::schema::open_db;

pub fn routes() -> Router {
    Router::new()
        .route("/", get(index_html))
        .route("/style.css", get(style_css))
        .route("/graph.js", get(graph_js))
        .route("/api/sessions", get(get_sessions))
        .route("/api/commands", get(get_commands))
        .route("/api/graph", get(get_graph_data))
        .route("/api/stats", get(get_stats))
        .route("/api/search", get(search))
}

async fn index_html() -> Html<&'static str> {
    Html(include_str!("static/index.html"))
}

async fn style_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css")],
        include_str!("static/style.css"),
    )
}

async fn graph_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/javascript")],
        include_str!("static/graph.js"),
    )
}

#[derive(Deserialize)]
struct SessionsQuery {
    limit: Option<usize>,
    offset: Option<usize>,
}

async fn get_sessions(Query(q): Query<SessionsQuery>) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = open_db().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let sessions = queries::get_sessions(&conn, q.limit.unwrap_or(200), q.offset.unwrap_or(0))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut enriched = Vec::new();
    for session in &sessions {
        let cmds = queries::get_session_commands(&conn, &session.id)
            .unwrap_or_default();
        let command_count = cmds.len();
        let has_failures = cmds.iter().any(|c| c.exit_code.is_some_and(|code| code != 0));
        let failure_count = cmds.iter().filter(|c| c.exit_code.is_some_and(|code| code != 0)).count();
        let mut repos: Vec<String> = cmds.iter().filter_map(|c| c.git_repo.clone()).collect();
        repos.sort();
        repos.dedup();
        let mut branches: Vec<String> = cmds.iter().filter_map(|c| c.git_branch.clone()).collect();
        branches.sort();
        branches.dedup();

        enriched.push(json!({
            "id": session.id,
            "start_time": session.start_time,
            "end_time": session.end_time,
            "terminal_app": session.terminal_app,
            "initial_dir": session.initial_dir,
            "command_count": command_count,
            "has_failures": has_failures,
            "failure_count": failure_count,
            "repos": repos,
            "branches": branches,
        }));
    }

    Ok(Json(json!({ "sessions": enriched })))
}

#[derive(Deserialize)]
struct CommandsQuery {
    session_id: Option<String>,
    limit: Option<usize>,
}

async fn get_commands(Query(q): Query<CommandsQuery>) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = open_db().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let commands = if let Some(session_id) = q.session_id {
        queries::get_session_commands(&conn, &session_id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    } else {
        queries::get_all_commands(&conn, q.limit.unwrap_or(100))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    Ok(Json(json!({ "commands": commands })))
}

async fn get_stats() -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = open_db().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let sessions = queries::get_sessions(&conn, 100_000, 0)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let all_commands = queries::get_all_commands(&conn, 1_000_000)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let session_count = sessions.len();
    let command_count = all_commands.len();
    let failure_count = all_commands.iter()
        .filter(|c| c.exit_code.is_some_and(|code| code != 0))
        .count();

    let mut repos: Vec<String> = all_commands.iter()
        .filter_map(|c| c.git_repo.clone())
        .collect();
    repos.sort();
    repos.dedup();
    let repo_count = repos.len();

    Ok(Json(json!({
        "sessions": session_count,
        "commands": command_count,
        "repos": repo_count,
        "failures": failure_count,
        "repo_names": repos,
    })))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    limit: Option<usize>,
}

async fn search(Query(sq): Query<SearchQuery>) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = open_db().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let results = queries::search_commands(&conn, &sq.q, sq.limit.unwrap_or(50))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let commands: Vec<serde_json::Value> = results.iter().map(|r| {
        json!({
            "id": r.command.id,
            "session_id": r.command.session_id,
            "command_text": r.command.command_text,
            "timestamp": r.command.timestamp,
            "duration_ms": r.command.duration_ms,
            "cwd": r.command.cwd,
            "git_repo": r.command.git_repo,
            "git_branch": r.command.git_branch,
            "exit_code": r.command.exit_code,
            "rank": r.rank,
        })
    }).collect();

    Ok(Json(json!({ "results": commands })))
}

async fn get_graph_data() -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = open_db().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let sessions = queries::get_sessions(&conn, 500, 0)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Collect per-repo stats and per-session repo sets
    let mut repo_commands: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut repo_sessions: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut repo_failures: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut repo_last_active: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let mut repo_branches: std::collections::HashMap<String, std::collections::HashSet<String>> = std::collections::HashMap::new();
    let mut session_repos: Vec<Vec<String>> = Vec::new();

    for session in &sessions {
        if let Ok(cmds) = queries::get_session_commands(&conn, &session.id) {
            let mut repos_in_session: Vec<String> = cmds
                .iter()
                .filter_map(|c| c.git_repo.clone())
                .collect();
            repos_in_session.sort();
            repos_in_session.dedup();

            for repo in &repos_in_session {
                let name = repo.rsplit('/').next().unwrap_or(repo).to_string();
                let cmd_count = cmds.iter().filter(|c| c.git_repo.as_deref() == Some(repo)).count();
                let fail_count = cmds.iter()
                    .filter(|c| c.git_repo.as_deref() == Some(repo) && c.exit_code.is_some_and(|code| code != 0))
                    .count();

                *repo_commands.entry(name.clone()).or_insert(0) += cmd_count;
                *repo_sessions.entry(name.clone()).or_insert(0) += 1;
                *repo_failures.entry(name.clone()).or_insert(0) += fail_count;

                let last = repo_last_active.entry(name.clone()).or_insert(0);
                if session.start_time > *last {
                    *last = session.start_time;
                }

                let branches = repo_branches.entry(name.clone()).or_default();
                for c in &cmds {
                    if c.git_repo.as_deref() == Some(repo) {
                        if let Some(ref b) = c.git_branch {
                            branches.insert(b.clone());
                        }
                    }
                }
            }

            let names: Vec<String> = repos_in_session
                .iter()
                .map(|r| r.rsplit('/').next().unwrap_or(r).to_string())
                .collect();
            session_repos.push(names);
        }
    }

    // Build nodes
    let nodes: Vec<serde_json::Value> = repo_commands
        .iter()
        .map(|(name, &cmds)| {
            let sess = repo_sessions.get(name).copied().unwrap_or(0);
            let fails = repo_failures.get(name).copied().unwrap_or(0);
            let last = repo_last_active.get(name).copied().unwrap_or(0);
            let branches: Vec<String> = repo_branches
                .get(name)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default();

            json!({
                "id": name,
                "label": name,
                "commands": cmds,
                "sessions": sess,
                "failures": fails,
                "last_active": last,
                "branches": branches,
            })
        })
        .collect();

    // Build edges: repos that co-occur in the same session
    let mut edge_map: std::collections::HashMap<(String, String), usize> = std::collections::HashMap::new();
    for repos in &session_repos {
        for i in 0..repos.len() {
            for j in (i + 1)..repos.len() {
                let a = repos[i].clone();
                let b = repos[j].clone();
                let key = if a < b { (a, b) } else { (b, a) };
                *edge_map.entry(key).or_insert(0) += 1;
            }
        }
    }

    let edges: Vec<serde_json::Value> = edge_map
        .iter()
        .map(|((a, b), &count)| {
            json!({
                "source": a,
                "target": b,
                "shared_sessions": count,
            })
        })
        .collect();

    Ok(Json(json!({
        "nodes": nodes,
        "edges": edges,
    })))
}
