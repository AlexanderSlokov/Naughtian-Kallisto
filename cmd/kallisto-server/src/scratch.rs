fn main() {
    let body = b"{\"data\":{\"value\":1},\"versions\":[1,2]}";
    let lazy = sonic_rs::get(body, &["data"]).unwrap();
    println!("{}", lazy.as_raw_str());
}
