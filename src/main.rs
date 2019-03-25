mod parser;

fn main() {
    let mut parser = parser::Parser::new();
    parser.feed('\x1b');
    parser.feed('\x18');
    parser.feed('\x21');
    println!("{:?}", parser);
}