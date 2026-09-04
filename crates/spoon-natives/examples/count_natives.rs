fn main() {
    let r = spoon_natives::bootstrap();
    let mut names = r.names();
    names.sort();
    println!("bootstrap natives: {}", names.len());
    for chunk in names.chunks(8) {
        println!(
            "  {}",
            chunk
                .iter()
                .map(|n| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}
