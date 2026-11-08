//! RIDL module registration and link checks (section 24.5).
//!
//! Every library with a RIDL surface registers its metadata blob from a
//! constructor. A module's imports are checked against providers as both
//! register, whichever comes first.

use klf_ridl::meta::{LinkCheck, Metadata, check_import, conflicting_current};
use std::sync::Mutex;

#[cfg(unix)]
use klf_ridl::module::ItemKind;
#[cfg(unix)]
use klf_ridl::types::{Scalar, Type};
#[cfg(unix)]
use klf_ridl_wire::{Reply, Value};
#[cfg(unix)]
use std::sync::{Arc, OnceLock};

#[cfg(unix)]
static REMOTE_HOST: OnceLock<Result<Arc<klf_ridl_wire::transport::Connection>, String>> =
    OnceLock::new();

static MODULES: Mutex<Vec<Metadata>> = Mutex::new(Vec::new());

fn strict() -> bool {
    std::env::var("KLF_RIDL_STRICT").is_ok_and(|value| value == "1")
}

fn hex(uuid: &[u8]) -> String {
    uuid.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Reports every import of `importer` that `provider` fails, and ends the
/// process for a fatal one.
fn check(importer: &Metadata, provider: &Metadata) {
    for import in importer
        .imports
        .iter()
        .filter(|import| import.module == provider.module_name)
    {
        match check_import(provider, &import.required, strict()) {
            LinkCheck::Ok => {}
            LinkCheck::RetiredWarning { proceed } => {
                eprintln!(
                    "warning KLF-RT-0201: {} requires retired ABI {} of {}",
                    importer.module_name,
                    hex(&import.required),
                    provider.module_name
                );
                if !proceed {
                    std::process::exit(1);
                }
            }
            LinkCheck::Missing => {
                eprintln!(
                    "runtime error KLF-RT-0200: {} requires ABI {} of {}, which provides {}",
                    importer.module_name,
                    hex(&import.required),
                    provider.module_name,
                    provider
                        .current_uuid()
                        .map_or_else(|| "none".to_owned(), |uuid| hex(&uuid))
                );
                std::process::exit(1);
            }
        }
    }
}

/// Registers the RIDL metadata blob of a loaded module.
///
/// # Safety
/// `blob` points to `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfrt_ridl_register(blob: *const u8, length: usize) {
    // SAFETY: the caller passes its own static metadata section.
    let bytes = unsafe { std::slice::from_raw_parts(blob, length) };
    let metadata = match Metadata::decode(bytes) {
        Ok(metadata) => metadata,
        Err(error) => {
            eprintln!("runtime error KLF-RT-0200: unreadable RIDL metadata: {error}");
            std::process::exit(1);
        }
    };
    let mut modules = MODULES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for loaded in modules.iter() {
        if conflicting_current(loaded, &metadata) {
            eprintln!(
                "runtime error KLF-RT-0202: two copies of {} with different current ABIs are loaded",
                metadata.module_name
            );
            std::process::exit(1);
        }
        check(&metadata, loaded);
        check(loaded, &metadata);
    }
    modules.push(metadata);
}

/// The compiler-generated native RIDL adapter's scalar-only remote entry.
/// The signature is the original RIDL declaration, not a second wire schema.
/// A nonzero result traps in the generated adapter; nonthrowing RIDL imports
/// cannot translate a remote failure into a legitimate return value.
///
/// # Safety
/// Paths and contract are readable UTF-8 arrays of their given lengths;
/// `words` points to `count` u64 values; `result` is writable for one u64.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfrt_ridl_remote_scalar(
    path: *const u8,
    path_length: u64,
    contract: *const u8,
    contract_length: u64,
    words: *const u64,
    count: u64,
    result: *mut u64,
) -> i32 {
    #[cfg(unix)]
    {
        let invocation = || {
            let path = unsafe { bounded_bytes(path, path_length, 1024)? };
            let contract = unsafe { bounded_bytes(contract, contract_length, 64 * 1024)? };
            let path = std::str::from_utf8(path).map_err(|_| ())?;
            let contract = std::str::from_utf8(contract).map_err(|_| ())?;
            if count > 128 || result.is_null() || (count != 0 && words.is_null()) {
                return Err(());
            }
            let args = if count == 0 {
                &[]
            } else {
                // SAFETY: generated adapters pass their own word buffer.
                unsafe { std::slice::from_raw_parts(words, count as usize) }
            };
            let result_word = remote_scalar_call(path, contract, args)?;
            // SAFETY: generated adapter gives a writable u64 slot.
            unsafe { result.write(result_word) };
            Ok(())
        };
        return match std::panic::catch_unwind(std::panic::AssertUnwindSafe(invocation)) {
            Ok(Ok(())) => 0,
            _ => -1,
        };
    }
    #[cfg(not(unix))]
    {
        let _ = (
            path,
            path_length,
            contract,
            contract_length,
            words,
            count,
            result,
        );
        -1
    }
}

#[cfg(unix)]
unsafe fn bounded_bytes<'a>(pointer: *const u8, length: u64, limit: usize) -> Result<&'a [u8], ()> {
    let length = usize::try_from(length).map_err(|_| ())?;
    if pointer.is_null() || length > limit {
        return Err(());
    }
    // SAFETY: the caller is compiler-generated code with statically allocated
    // buffers. The borrowed view is used only within the FFI invocation.
    Ok(unsafe { std::slice::from_raw_parts(pointer, length) })
}

#[cfg(unix)]
fn remote_scalar_call(path: &str, contract: &str, words: &[u64]) -> Result<u64, ()> {
    let connection = REMOTE_HOST
        .get_or_init(|| {
            klf_helper_apps::attached_connection()
                .map_err(|error| error.to_string())?
                .map(Arc::new)
                .ok_or_else(|| "no inherited RIDL host channel".to_owned())
        })
        .as_ref()
        .map_err(|_| ())?;
    remote_scalar_call_on(connection, path, contract, words)
}

#[cfg(unix)]
fn remote_scalar_call_on(
    connection: &klf_ridl_wire::transport::Connection,
    path: &str,
    contract: &str,
    words: &[u64],
) -> Result<u64, ()> {
    let (module, name) = path.split_once('.').ok_or(())?;
    let declaration =
        klf_ridl::parse(klf_core::FileId::from_u32(0), module, contract).map_err(|_| ())?;
    let function = declaration
        .items
        .iter()
        .find_map(|item| match &item.kind {
            ItemKind::Function(function) if function.name == name => Some(function),
            _ => None,
        })
        .ok_or(())?;
    if !function.generics.is_empty()
        || function.self_param.is_some()
        || function.params.len() != words.len()
    {
        return Err(());
    }
    let arguments = function
        .params
        .iter()
        .zip(words)
        .map(|(parameter, word)| {
            if parameter.convention != klf_ridl::module::Convention::Owned {
                return Err(());
            }
            let Type::Scalar(scalar) = &parameter.ty else {
                return Err(());
            };
            encode_remote_scalar(*scalar, *word)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let reply = connection.call(path, arguments).map_err(|_| ())?;
    let Reply::Returned(value) = reply else {
        return Err(());
    };
    match function.result.as_ref() {
        None | Some(Type::Scalar(Scalar::Void)) if value == Value::Void => Ok(0),
        Some(Type::Scalar(scalar)) => decode_remote_scalar(*scalar, value),
        _ => Err(()),
    }
}

#[cfg(unix)]
fn encode_remote_scalar(scalar: Scalar, bits: u64) -> Result<Value, ()> {
    match scalar {
        Scalar::I8 | Scalar::I16 | Scalar::I32 | Scalar::I64 => {
            let shift = match scalar {
                Scalar::I8 => 56,
                Scalar::I16 => 48,
                Scalar::I32 => 32,
                _ => 0,
            };
            Ok(Value::Int((((bits << shift) as i64) >> shift) as i128))
        }
        Scalar::U8 | Scalar::U16 | Scalar::U32 | Scalar::U64 => {
            let bits = match scalar {
                Scalar::U8 => bits & 255,
                Scalar::U16 => bits & 65535,
                Scalar::U32 => bits & 0xffff_ffff,
                _ => bits,
            };
            Ok(Value::Uint(u128::from(bits)))
        }
        Scalar::Bool => Ok(Value::Bool(bits != 0)),
        Scalar::F32 => Ok(Value::Float(
            f64::from(f32::from_bits(bits as u32)).to_bits(),
        )),
        Scalar::F64 => Ok(Value::Float(bits)),
        _ => Err(()),
    }
}

#[cfg(unix)]
fn decode_remote_scalar(scalar: Scalar, value: Value) -> Result<u64, ()> {
    match (scalar, value) {
        (Scalar::I8 | Scalar::I16 | Scalar::I32 | Scalar::I64, Value::Int(number)) => {
            let width = match scalar {
                Scalar::I8 => 8,
                Scalar::I16 => 16,
                Scalar::I32 => 32,
                _ => 64,
            };
            let range = (-(1_i128 << (width - 1)))..=((1_i128 << (width - 1)) - 1);
            if range.contains(&number) {
                Ok(number as i64 as u64)
            } else {
                Err(())
            }
        }
        (Scalar::U8 | Scalar::U16 | Scalar::U32 | Scalar::U64, Value::Uint(number)) => {
            let width = match scalar {
                Scalar::U8 => 8,
                Scalar::U16 => 16,
                Scalar::U32 => 32,
                _ => 64,
            };
            if number <= ((1_u128 << width) - 1) {
                Ok(number as u64)
            } else {
                Err(())
            }
        }
        (Scalar::Bool, Value::Bool(flag)) => Ok(u64::from(flag)),
        (Scalar::F32, Value::Float(bits)) => Ok(u64::from((f64::from_bits(bits) as f32).to_bits())),
        (Scalar::F64, Value::Float(bits)) => Ok(bits),
        _ => Err(()),
    }
}

#[cfg(all(unix, test))]
mod remote_tests {
    use super::*;
    use klf_ridl_wire::transport::Connection;

    #[test]
    fn scalar_ridl_values_preserve_signedness_and_float_bits() {
        assert_eq!(
            encode_remote_scalar(Scalar::I8, 0xff).unwrap(),
            Value::Int(-1)
        );
        assert_eq!(
            decode_remote_scalar(Scalar::I8, Value::Int(-1)),
            Ok(u64::MAX)
        );
        assert_eq!(decode_remote_scalar(Scalar::I8, Value::Int(256)), Err(()));
        assert_eq!(
            encode_remote_scalar(Scalar::U16, 0xffff).unwrap(),
            Value::Uint(65535)
        );
        assert_eq!(
            decode_remote_scalar(Scalar::U16, Value::Uint(65536)),
            Err(())
        );
        let bits = (3.5_f32).to_bits() as u64;
        let encoded = encode_remote_scalar(Scalar::F32, bits).unwrap();
        assert_eq!(decode_remote_scalar(Scalar::F32, encoded), Ok(bits));
        assert_eq!(decode_remote_scalar(Scalar::Bool, Value::Bool(true)), Ok(1));
    }

    #[test]
    fn native_remote_scalar_bridge_calls_host_through_ridl() {
        let (host, guest) = Connection::pair(123).unwrap();
        let serving = std::thread::spawn(move || {
            host.serve_once(|call| {
                assert_eq!(call.function, "Harmony.subtract");
                assert_eq!(call.arguments, vec![Value::Int(-2), Value::Int(40)]);
                Reply::Returned(Value::Int(-42))
            })
            .unwrap();
        });
        let answer = remote_scalar_call_on(
            &guest,
            "Harmony.subtract",
            "function subtract(a: I32, b: I32) -> I32\n",
            &[(-2_i64) as u64, 40],
        );
        assert_eq!(answer, Ok((-42_i64) as u64));
        serving.join().unwrap();
    }

    #[test]
    fn native_remote_scalar_bridge_rejects_widened_result() {
        let (host, guest) = Connection::pair(124).unwrap();
        let serving = std::thread::spawn(move || {
            host.serve_once(|_| Reply::Returned(Value::Uint(256)))
                .unwrap();
        });
        assert_eq!(
            remote_scalar_call_on(&guest, "Harmony.byte", "function byte() -> U8\n", &[]),
            Err(())
        );
        serving.join().unwrap();
    }
}
