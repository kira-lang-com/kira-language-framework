//! `klf.trap.operands` on the VM: the values the next trap's report names
//! as `{0}` and `{1}`, filled in when the trap stops the run, as the native
//! runtime does (`klf-native-rt` `trap.rs`).

use std::cell::Cell;

thread_local! {
    static OPERANDS: Cell<Option<[i64; 2]>> = const { Cell::new(None) };
    static TEXT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn register(capabilities: &mut crate::capabilities::Capabilities) {
    capabilities.register("klfsvc_trap_operands", |arguments| {
        let word = |index: usize| {
            arguments
                .get(index)
                .map_or(0, |value| i64::from_ne_bytes(value.to_ne_bytes()))
        };
        OPERANDS.with(|operands| operands.set(Some([word(0), word(1)])));
        0
    });
    capabilities.register_with_memory("klfsvc_trap_text", |arguments, memory| {
        let [address, length] = arguments else {
            return u64::MAX;
        };
        let Some(count) = usize::try_from(*length)
            .ok()
            .filter(|count| *count <= klf_rt::MAX_SERVICE_BUFFER_BYTES)
        else {
            return u64::MAX;
        };
        let mut bytes = vec![0; count];
        if count > 0 && !memory.read_buffer(*address, &mut bytes) {
            return u64::MAX;
        }
        TEXT.with(|text| *text.borrow_mut() = Some(String::from_utf8_lossy(&bytes).into_owned()));
        0
    });
}

/// A trap's code or report with the recorded operands in place.
pub(crate) fn filled(code: &str) -> String {
    let operands = OPERANDS.with(Cell::take);
    let text = TEXT.with(|text| text.borrow_mut().take());
    if klf_diag::report::report_code(code).is_none() {
        return code.to_owned();
    }
    let valued = klf_diag::report::fill_operands(code, &operands.map(|pair| pair.to_vec()).unwrap_or_default());
    klf_diag::report::fill_text(&valued, text.as_deref())
}
