// vim: set tw=99 ts=4 sts=4 sw=4 et:

//use std::collections::HashSet;
use std::io::{Read, stdin};

use nom::error::convert_error;

use cachet_cacheir_translator::parser::parse;
use cachet_cacheir_translator::translator::translate;

fn main() {
    let mut input = String::new();
    stdin().read_to_string(&mut input).unwrap();
    let stub = match parse(&input) {
        Ok(stub) => stub,
        Err(err) => {
            eprintln!("{}", convert_error(input.as_str(), err));
            panic!("parse error");
        }
    };
    //println!("{:#?}\n", &stub);
    let mod_ = translate(&stub);
    //println!("{:#?}\n\n{}", mod_, mod_);
    println!("{}", mod_);
}
