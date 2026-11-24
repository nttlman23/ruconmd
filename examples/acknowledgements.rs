//! Печатает лицензии встроенных описаний языков. Нужен при сборке выпуска.

fn main() {
    let listing = two_face::acknowledgement::listing();
    let licenses = listing.for_syntaxes();
    for license in licenses {
        if !license.needs_acknowledgement() {
            continue;
        }
        let mut text = String::new();
        license.write_md(&mut text);
        println!("{text}");
    }
}
