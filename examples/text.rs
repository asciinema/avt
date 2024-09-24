use avt::util::TextCollector;
use avt::Vt;

fn main() {
    let vt = Vt::builder()
        .size(1000, 100)
        .resizable(true)
        .scrollback_limit(100)
        .build();

    let input = std::io::stdin();
    let mut collector = TextCollector::new(vt);
    let mut line = String::new();

    while let Ok(n) = input.read_line(&mut line) {
        if n == 0 {
            break;
        };

        for l in collector.feed_str(&line) {
            println!("{}", l);
        }

        line = String::new();
    }

    for l in collector.flush() {
        println!("{}", l);
    }
}
