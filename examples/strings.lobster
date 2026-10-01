// Strings, chars, escapes, references.
// Every item here must pass `lobster check`.

fn greet(name: &str) -> &str {
    name
}

fn main() {
    let hello = "hi\n";
    let tabbed = "a\tb";
    let quote = "say \"hi\"";
    let a = 'a';
    let e = 'é';
    let hex = '\x41';
    let emoji = '\u{1F600}';
    let newline = '\n';
    println(hello);
    println(greet(tabbed));
}
