mod utils;

use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use axum::{
    body::StreamBody,
    extract,
    http::{header, Request, Response, StatusCode},
    routing::*,
    Extension, Router,
};
use clap::Parser;
use sqlx::{Database, Row, sqlite::SqlitePoolOptions, Sqlite};
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

#[derive(Debug, Clone)]
struct AppState<'a> {
    store_file_path: &'a str,
    db: Arc<sqlx::Pool<sqlx::Any>>,
}

#[tokio::main]
async fn main() {
    let arg = Config::parse();
    
    let mut db = sqlx::AnyPool::connect_lazy(&format!("sqlite://{}", arg.db_path)).unwrap();

    let state = AppState {
        store_file_path: arg.store_file_path.to_owned().as_ref(),
        db: Arc::new(db),
    };
    let app = Router::new()
        .route("/upload", post(upload))
        .route("/download/:id", get(download))
        .with_state(state);

    let result = axum::Server::bind(&arg.bind.parse().unwrap())
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
    Extension(mut state): Extension<AppState<'_>>,
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
                .fetch_one(&*state.db)
                .await
            {
                Ok(row) => {
                    let c = row.get(0) as i32;
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
                x if utils::check_token(&x, 6, 32) => x,
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

            let store_file_name = utils::hashed_filename(file_name);
            fs::write(Path::new(state.store_file_path).join(file_name), file)
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

async fn download(
    id: String,
    state: Extension<AppState<'_>>,
    req: DownloadQuery,
) -> Result<Response<StreamBody<impl Future>>, (StatusCode, String)> {
    if !utils::check_token(&req.token.unwrap_or_default(), 6, 32) {
        return Err((StatusCode::NOT_FOUND, "404 Not Found".to_string()));
    }

    let file = match sqlx::query("SELECT name FROM files WHERE store_name = ? AND token = ?")
        .bind(id)
        .bind(req.token)
        .fetch_one(&*state.db)
        .await
    {
        Ok(x) => x.get(0) as String,
        _ => {
            return Err((StatusCode::NOT_FOUND, "404 Not Found".to_string()));
        }
    };
    let path = Path::new(state.store_file_path).join(file);
    fs::try_exists(path)
        .await
        .map(|_| {
            let body = fs::read(path);
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
