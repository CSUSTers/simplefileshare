#![feature(io_error_other)]

mod utils;

use std::path::Path;
use std::sync::Arc;

use axum::{
    body::StreamBody,
    extract::{self, DefaultBodyLimit, Multipart, Query, State},
    http::{header, HeaderMap, Response, StatusCode},
    routing::*,
    Json, Router,
};
use clap::Parser;

use futures_util::TryStreamExt;
use sqlx::Row;
use tokio::{fs, io::AsyncRead};
use tokio_util::io::{ReaderStream, StreamReader};

#[derive(Debug, Clone, Parser)]
#[command(name = "simplefileshare", author, version, long_about = None)]
struct Config {
    #[arg(long = "db", default_value = "./db.sqlite")]
    db_path: String,

    #[arg(long, short, default_value = "127.0.0.1:8080")]
    bind: String,

    #[arg(long = "store", short, default_value = "./store")]
    store_file_path: String,

    // 最大文件大小，单位MB
    #[arg(long, short, default_value = "10")]
    max_file_size: usize,
}

#[derive(Debug, Clone)]
struct AppState {
    store_file_path: String,
    db: sqlx::Pool<sqlx::Any>,
    max_upload_content_length: i64,
}

#[tokio::main]
async fn main() {
    let arg = Config::parse();

    let db_opt = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&arg.db_path)
        .create_if_missing(true);
    let db = sqlx::AnyPool::connect_lazy_with(db_opt.into());

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users
        (
            id      INTEGER PRIMARY KEY AUTOINCREMENT,
            uuid    TEXT    NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1
        );"#,
    )
    .execute(&db)
    .await
    .unwrap();

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS files
        (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            name       TEXT    NOT NULL,
            token      TEXT    NOT NULL,
            user_uuid  TEXT    NOT NULL,
            store_name TEXT    NOT NULL,
            created_at INTEGER DEFAULT NULL,
            dead_at    INTEGER DEFAULT NULL,
            avaiable   INTEGER NOT NULL DEFAULT 1
        );"#,
    )
    .execute(&db)
    .await
    .unwrap();

    std::fs::create_dir_all(&arg.store_file_path).unwrap();

    let state = AppState {
        store_file_path: arg.store_file_path.to_owned(),
        db,
        max_upload_content_length: (arg.max_file_size as i64) * (1 << 20),
    };
    let app = Router::new()
        .route("/upload", post(upload))
        .route("/download/:id", get(download))
        .with_state(Arc::new(state))
        .layer(DefaultBodyLimit::max(arg.max_file_size * 1024 * 1024));

    println!("Listening on http://{}", arg.bind);
    let (exit_tx, mut exit_rx) = tokio::sync::broadcast::channel(1);

    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.expect("recv signal error");
        println!("received CTRL-C signal");
        exit_tx.send(true).expect("notify threads to exit error");
    });
    let result = axum::Server::bind(&arg.bind.parse().unwrap())
        .serve(app.into_make_service())
        .with_graceful_shutdown(async move { while exit_rx.recv().await.unwrap_or(false) {} })
        .await;
    match result {
        Ok(_) => println!("server exited"),
        Err(e) => println!("Error: {}", e),
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct UploadQuery {
    token: Option<String>,
    live: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct UploadResponse {
    token: String,
    id: String,
}

#[axum::debug_handler]
async fn upload(
    header: HeaderMap,
    Query(param): Query<UploadQuery>,
    State(state): State<Arc<AppState>>,
    mut body: Multipart,
) -> Result<Json<UploadResponse>, (StatusCode, String)> {
    if let Some(user) = header.get("user-uuid") {
        let err_403 = (StatusCode::FORBIDDEN, "403 For Biden".to_owned());
        let err_400 = (StatusCode::BAD_REQUEST, "400 Bad Request".to_owned());
        let err_413 = (StatusCode::PAYLOAD_TOO_LARGE, "File too LARGE".to_owned());
        let err_500 = (
            StatusCode::INTERNAL_SERVER_ERROR,
            "500 Server Internal Error".to_owned(),
        );

        let user = user
            .to_str()
            .or(Err(()))
            .and_then(|u| if utils::check_uuid(u) { Ok(u) } else { Err(()) })
            .map_err(|_| err_403.to_owned())?;

        match sqlx::query("SELECT count(id) FROM users WHERE uuid = ? AND enabled = 1")
            .bind(user)
            .fetch_one(&state.db)
            .await
        {
            Ok(row) => {
                let c: i32 = row.get(0);
                if c == 0 {
                    return Err(err_403);
                }
            }
            Err(_) => return Err(err_500),
        }

        if let Some(content_length_header) = header.get(axum::http::header::CONTENT_LENGTH) {
            content_length_header
                .to_str()
                .or(Err(err_400.to_owned()))
                .and_then(|s| {
                    if s.trim().is_empty() {
                        Ok(())
                    } else {
                        s.parse().or(Err(err_400.to_owned())).and_then(|x: i64| {
                            if x > state.max_upload_content_length {
                                Err(err_413)
                            } else {
                                Ok(())
                            }
                        })
                    }
                })?;
        }

        let token = match param.token.unwrap_or_default() {
            x if utils::check_token(&x, 6, 32) => x,
            _ => utils::ramdom_string(12),
        };

        let now = chrono::Utc::now().timestamp_millis();

        let dead_at = match param.live {
            Some(x) if x > 0 && x < chrono::Duration::days(365 * 10).num_milliseconds() => {
                Some(now + x)
            }
            _ => None,
        };

        let file = match body.next_field().await {
            Ok(Some(x)) => x,
            _ => return Err(err_400),
        };
        let file_name = file.file_name().unwrap_or_default().to_owned();
        if file_name.is_empty() || file_name.len() > 255 {
            return Err(err_400);
        }

        let store_file_name = utils::hashed_filename(&file_name);
        let store_file_path = Path::new(&state.store_file_path).join(&store_file_name);
        let mut f = fs::File::create(&store_file_path).await.unwrap();

        let mut reader = StreamReader::new(file.map_err(tokio::io::Error::other));

        let remove_file_fn = || {
            std::fs::remove_file(&store_file_path).unwrap_or_else(|e| {
                println!(
                    "remove file '{}' error: {}",
                    store_file_path.to_str().unwrap_or("???"),
                    e
                );
            });
        };

        tokio::io::copy_buf(&mut reader, &mut f)
            .await
            .map_err(|_| {
                remove_file_fn();
                err_400.to_owned()
            })?;

        sqlx::query("INSERT INTO files (name, token, user_uuid, store_name, created_at, dead_at) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(&file_name)
            .bind(&token)
            .bind(user)
            .bind(&store_file_name)
            .bind(now)
            .bind(dead_at)
            .execute(&state.db)
            .await
            .map_err(|_| {
                remove_file_fn();
                err_500.to_owned()
            })?;

        return Ok(UploadResponse {
            token,
            id: store_file_name,
        }
        .into());
    }
    Err((StatusCode::UNAUTHORIZED, "Unauthorized".to_string()))
}

#[derive(Debug, Clone, serde::Deserialize)]
struct DownloadQuery {
    token: Option<String>,
}

#[axum::debug_handler]
async fn download(
    extract::Path(id): extract::Path<String>,
    Query(mut req): Query<DownloadQuery>,
    State(state): State<Arc<AppState>>,
) -> Result<Response<StreamBody<ReaderStream<impl AsyncRead>>>, (StatusCode, String)> {
    let token = req.token.take().unwrap_or_default();

    if !utils::check_token(&token, 6, 32) {
        return Err((StatusCode::NOT_FOUND, "404 Not Found".to_owned()));
    }

    let filename = match sqlx::query("SELECT name FROM files WHERE store_name = ? AND token = ? AND avaiable = true")
        .bind(id.as_str())
        .bind(&token)
        .fetch_one(&state.db)
        .await
    {
        Ok(x) => x.get::<String, _>(0),
        _ => {
            return Err((StatusCode::NOT_FOUND, "404 Not Found".to_string()));
        }
    };
    let path = Path::new(&state.store_file_path).join(&id);
    fs::try_exists(&path)
        .await
        .and(fs::File::open(&path).await)
        .map(move |file| {
            let stream = tokio_util::io::ReaderStream::new(file);
            let body = StreamBody::new(stream);
            Response::builder()
                .status(StatusCode::OK)
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", &filename),
                )
                .body(body)
                .unwrap()
        })
        .map_err(|_| (StatusCode::NOT_FOUND, "404 Not Found".to_string()))
}
