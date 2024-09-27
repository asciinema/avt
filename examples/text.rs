use avt::util::TextCollector;
use avt::Vt;

fn main() {
    let vt = Vt::builder()
        .size(1000, 100)
        .resizable(true)
        .scrollback_limit(100)
        .build();

    let input = std::io::stdin();
    let mut buf = String::new();
    let mut collector = TextCollector::new(vt);

    while let Ok(n) = input.read_line(&mut buf) {
        if n == 0 {
            break;
        };

        for line in collector.feed_str(&buf) {
            println!("{}", line);
        }

        buf.clear();
    }

    for line in collector.flush() {
        println!("{}", line);
    }
}
