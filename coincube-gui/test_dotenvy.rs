fn main() {
    if let Err(e) = dotenvy::dotenv() {
        if e.not_found() {
            println!("Not found");
        }
    }
}
