// path: crates/polygloss-app/src/fixture.rs
//
// `#[cfg(test)]` items are stripped by brace matching: a declaration
// strips only itself, a module its whole block (braces in strings and
// comments included). Findings: `px(12.)` and `px(15.)` only.

#[cfg(test)]
mod tests;

fn real() -> Pixels {
    px(12.)
}

#[cfg(test)]
#[allow(dead_code)]
mod inline {
    fn f() -> Pixels {
        px(13.)
    }
    fn g() {
        let s = "}";
        let c = '}';
        // }
        let w = 14.0;
    }
}

fn after() -> Pixels {
    px(15.)
}
