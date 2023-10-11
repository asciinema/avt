use avt::Vt;

fn main() {
    let mut vt = Vt::new(1000, 100);
    let mut line = String::new();
    let input = std::io::stdin();

    while let Ok(n) = input.read_line(&mut line) {
        if n == 0 {
            break;
        };

        vt.feed_str(&line);
        line = String::new();
    }

    let mut text = vt.text();

    while !text.is_empty() && text[text.len() - 1].is_empty() {
        text.truncate(text.len() - 1);
    }

    println!("{}", text.join("\n"));
}
