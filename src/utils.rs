use lazy_static::lazy_static;
use rand::{self, Rng};
use xxhash_rust as xxh;

pub fn ramdom_string(len: usize) -> String {
    const ALPHAS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| ALPHAS.as_bytes()[rng.gen_range(0..ALPHAS.len())] as char)
        .collect()
}

pub fn hashed_filename(s: &str) -> String {
    let salt = rand::thread_rng().gen::<u64>();
    let h = xxh::xxh3::xxh3_128_with_seed(s.as_bytes(), salt);
    format!("{:x}", h)
}

pub fn check_uuid(uuid: &str) -> bool {
    lazy_static! {
        static ref PATT: regex::Regex = regex::Regex::new(
            r"^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$",
        )
        .unwrap();
    }
    PATT.is_match(uuid)
}

pub fn check_token(s: &str, min_len: usize, max_len: usize) -> bool {
    if s.len() < min_len || s.len() > max_len {
        return false;
    }
    let patt = regex::Regex::new(r"^[a-zA-Z0-9]+$").unwrap();
    patt.is_match(s)
}
