//! The source name of a symbol a backtrace or a crash report shows.
//!
//! A function a module exports carries its RIDL symbol (section 24.2), which
//! [`klf_ridl::demangle::demangle`] reads back. One it keeps to itself is
//! `kir.<name>`, where a name that two modules declare carries its module as
//! `Name#Module` and a function the build made for a declaration carries its
//! role as `name#role`. Every other symbol, the runtime's own and C's, is
//! shown as the platform spells it, without Mach-O's leading `_`.

#[must_use]
pub(crate) fn source_name(symbol: &str) -> String {
    if let Some(name) = klf_ridl::demangle::demangle(symbol) {
        return name;
    }
    let bare = symbol.strip_prefix('_').unwrap_or(symbol);
    match bare.strip_prefix("kir.") {
        Some(local) => klf_diag::names::local_source_name(local),
        None if symbol.starts_with("_kir") || bare.contains("::") => symbol.to_owned(),
        None => bare.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::source_name;

    #[test]
    fn symbols_read_as_their_source_names() {
        for (symbol, shown) in [
            ("_kir.pick#TrapGallery", "TrapGallery.pick"),
            ("kir.Picker#KiraUI<String>.describe", "KiraUI.Picker<String>.describe"),
            ("_kir.ExecutorTask.cancel#dispatch", "ExecutorTask.cancel (dispatch)"),
            ("_kir.Checker.checkCall", "Checker.checkCall"),
            ("_RIDL18Geometry5parseF", "Geometry.parse"),
            ("_main", "main"),
            ("klf_defsvc::drive::run_phase", "klf_defsvc::drive::run_phase"),
        ] {
            assert_eq!(source_name(symbol), shown, "{symbol}");
        }
    }
}
