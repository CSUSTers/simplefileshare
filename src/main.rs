mod utils;

use std::path::Path;
use std::sync::Arc;

use axum::{
    body::{StreamBody, Bytes},
    extract::{self, multipart::Field},
    http::{header, Request, Response, StatusCode},
    routing::*,
    Extension, Router,
};
use clap::Parser;
use futures_core::Stream;
use futures_util::TryStreamExt;
use sqlx::Row;
use tokio::{fs, io::AsyncRead};
use tokio_util::io::ReaderStream;

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
struct AppState {
    store_file_path: String,
    db: Arc<sqlx::Pool<sqlx::Any>>,
}

#[tokio::main]
async fn main() {
    let arg = Config::parse();

    let mut db = sqlx::AnyPool::connect_lazy(&format!("sqlite://{}", arg.db_path)).unwrap();

    let state = AppState {
        store_file_path: arg.store_file_path.to_owned(),
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
    Extension(mut state): Extension<AppState>,
    extract::Query(param): extract::Query<UploadQuery>,
    req: Request<extract::Multipart>,
) -> Result<Response<UploadResponse>, (StatusCode, String)> {
    if let Some(user) = req.headers().get("user-uuid") {
        let (parts, mut body) = req.into_parts();

        if let Some(user) = parts.headers.get("user-uuid") {
            let user = user.to_str().unwrap();
            if !utils::check_uuid(user) {
                return Err((StatusCode::FORBIDDEN, "403 For Biden".to_owned()));
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
                        "500 Server Error".to_owned(),
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
            if file_name.is_empty() || file_name.len() > 255 {
                return Err((StatusCode::BAD_REQUEST, "Bad Request".to_owned()));
            }

            let store_file_name = utils::hashed_filename(file_name);
            let f = fs::File::create(Path::new(&state.store_file_path).join(file_name))
                .await
                .unwrap();
            let stream = tokio_util::io::StreamReader::<(dyn Stream<Item = Result<Bytes, std::io::Error>> + Sized), _>::new(file.map_err(|e| async {e.into()}));
            tokio::io::copy(&mut stream, &mut f).await.map_err(|_| {
                (
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
            .execute(&*state.db)
            .await
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "500 Server Error".to_string(),
                )
            })?;

            return Ok(Response::builder()
                .status(200)
                .body(UploadResponse {
                    token,
                    id: store_file_name,
                })
                .unwrap());
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
    state: Extension<AppState>,
    req: DownloadQuery,
) -> Result<Response<StreamBody<ReaderStream<impl AsyncRead>>>, (StatusCode, String)>
{
    if !utils::check_token(&req.token.unwrap_or_default(), 6, 32) {
        return Err((StatusCode::NOT_FOUND, "404 Not Found".to_owned()));
    }

    let filename = match sqlx::query("SELECT name FROM files WHERE store_name = ? AND token = ?")
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
    let path = Path::new(&state.store_file_path).join(filename);
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
                    format!("attachment; filename=\"{}\"", filename),
                )
                .body(body)
                .unwrap()
        })
        .map_err(|_| (StatusCode::NOT_FOUND, "404 Not Found".to_string()))
}
