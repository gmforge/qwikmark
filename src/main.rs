mod qwikmark;

use qwikmark::qwikmark;
use std::{env, error::Error, fs};

fn main() -> Result<(), Box<dyn Error>> {
    // TODO: These need to be part of context
    // NOTE: Links cannot be nested
    // NOTE: Verbatim cannot be nested
    let args: Vec<String> = env::args().collect();
    let name = args[1].clone();
    let contents = fs::read_to_string(name)?;
    let document = qwikmark(&contents);
    println!("{:?}", document);
    Ok(())
}
