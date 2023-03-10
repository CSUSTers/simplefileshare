use rand::{self, Rng};
use xxhash_rust as xxh;

pub fn ramdom_string(len: usize) -> String {
    const alphas: &'static str = concat!('a'..='z', 'A'..='Z', '0'..='9').to_string();
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| alphas.chars().nth(rng.gen_range(0..alphas.len())).unwrap())
        .collect()
}

pub fn hashed_filename(s: &str) -> String {
    let salt = rand::thread_rng().gen::<u128>();
    let h = xxh::xxh3::xxh3_128_with_seed(s.as_bytes(), salt);
    format!("{:x}", h)
}

pub fn check_uuid(uuid: &str) -> bool {
    let patt = regex::Regex::new(
        r"^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$",
    )
    .unwrap();
    patt.is_match(uuid)
}

pub fn check_token(s: &str, min_len: usize, max_len: usize) -> bool {
    if s.len() < min_len || s.len() > max_len {
        return false;
    }
    let patt = regex::Regex::new(r"^[a-zA-Z0-9]+$").unwrap();
    patt.is_match(s)
}
