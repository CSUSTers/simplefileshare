mod utils;

use std::{collections::HashMap, env::join_paths, iter::Map, path::Path, sync::Arc};

use axum::{
    body::StreamBody,
    extract,
    http::{header, Request, Response, StatusCode},
    response,
    routing::*,
    Extension, Json, RequestExt, Router,
};
use clap::Parser;
use sqlx::{sqlx_macros, Database, Row};
use tokio::fs;

#[derive(Debug, Clone, Parser)]
#[command(name = "simplefileshare", author, version, long_about = None)]
struct Config {
    #[arg(long = "db", default_value = "./db.sqlite")]
    db_path: String,

    #[arg(long, short, default_value = ":8080")]
    bind: String,

    #[arg(long = "store", short, default_value = "./store")]
    store_file_path: String,
}

#[derive(Debug, Clone, Send)]
struct AppState {
    store_file_path: &str,
    db: Arc<Database>,
}

#[tokio::main]
async fn main() {
    let arg = Config::parse();

    let mut db = sqlx::SqlitePool::connect_lazy(&arg.db_path).await.unwrap();

    let state = AppState {
        store_file_path: arg.store_file_path.into(),
        db: Arc::new(db),
    };
    let app = Router::new()
        .route("/upload", post(upload))
        .route("/download/:id", get(download))
        .with_state(state);

    let result = axum::Server::bind(arg.bind.parse().unwrap())
        .serve(app.into_make_service())
        .await;
    match result {
        Ok(_) => {}
        Err(e) => {
            println!("Error: {}", e);
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct UploadQuery {
    token: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct UploadResponse {
    token: String,
    id: String,
}

async fn upload(
    Extension(mut state): Extension<AppState>,
    extract::Query(param): extract::Query<UploadQuery>,
    req: Request<extract::Multipart>,
) -> Result<Response<UploadResponse>, (StatusCode, String)> {
    if let Some(user) = req.headers().get("user-uuid") {
        let (parts, mut body) = req.into_parts();

        if let Some(user) = parts.headers.get("user-uuid") {
            let user = user.to_str().unwrap();
            if !utils::check_uuid(user) {
                return Err((StatusCode::FORBIDDEN, "403 For Biden".to_string()));
            }

            match sqlx::query("SELECT count(id) FROM users WHERE uuid = ? AND enabled = 1")
                .bind(user)
                .fetch_one(&mut state.db)
                .await
            {
                Ok(rows) => {
                    let c = (rows as Row).get(0) as i32;
                    if c == 0 {
                        return Err((StatusCode::FORBIDDEN, "403 For Biden".to_string()));
                    }
                }
                Err(_) => {
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "500 Server Error".to_string(),
                    ))
                }
            }

            let token = match param.token.unwrap_or_default() {
                x if utils::check_token(s, 6, 32) => x,
                _ => utils::ramdom_string(12),
            };

            let mut file = match body.next_field().await {
                Ok(Some(x)) => x,
                _ => {
                    return Err((StatusCode::BAD_REQUEST, "Bad Request".to_string()));
                }
            };
            let file_name = file.file_name().unwrap_or_default();
            if file_name.is_empty() || file_name > 255 {
                return Err((StatusCode::BAD_REQUEST, "Bad Request".to_string()));
            }

            let store_file_name = utils::hashed_filename(s);
            fs::write(Path::join(state.store_file_path, file_name), file)
                .await
                .map_err(|_| {
                    Error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "500 Server Error".to_string(),
                    )
                })?;
            sqlx::query(
                "INSERT INTO files (name, token, user_uuid, store_name) VALUES (?, ?, ?, ?)",
            )
            .bind(file_name)
            .bind(token)
            .bind(user)
            .bind(store_file_name)
            .execute(state.db)
            .await
            .map_err(|_| {
                Error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "500 Server Error".to_string(),
                )
            })?;

            Ok(Response::new(UploadResponse {
                token,
                id: store_file_name,
            }))
        }
    }
    Err((StatusCode::UNAUTHORIZED, "Unauthorized".to_string()))
}

#[derive(Debug, Clone, serde::Deserialize)]
struct DownloadQuery {
    token: Option<String>,
}

async fn dowload(
    id: String,
    state: Extension<AppState>,
    req: Request<Body>,
) -> Result<Response<StreamBody>, (StatusCode, String)> {
    if !utils::check_token(s, 6, 32) {
        return Err((StatusCode::NOT_FOUND, "404 Not Found".to_string()));
    }

    let file = match sqlx::query("SELECT name FROM files WHERE store_name = ? AND token = ?")
        .bind(id)
        .bind(token)
        .fetch_one(state.db)
        .await as Result<Row, _>
    {
        Ok(x) => x.get(0) as String,
        _ => {
            return Err((StatusCode::NOT_FOUND, "404 Not Found".to_string()));
        }
    };
    let path = Path::join(state.store_file_path, file);
    fs::try_exists(path)
        .await
        .map(|_| {
            let body = fs::read(fs);
            Response::builder()
                .status(StatusCode::OK)
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", file),
                )
                .body(body)
                .unwrap()
        })
        .map_err(|_| Error((StatusCode::NOT_FOUND, "404 Not Found".to_string())))
}
