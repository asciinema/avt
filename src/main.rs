mod vt;

fn main() {
    let mut vt = vt::VT::new();
    vt.feed('\x1b');
    vt.feed('\x18');
    vt.feed('\x21');
    println!("{:?}", vt);
}
