// Copyright 2021-2023, Offchain Labs, Inc.
// For license information, see https://github.com/nitro/blob/master/LICENSE

mod value;

use crate::value::*;
use arbutil::wavm;
use fnv::FnvHashSet as HashSet;
use go_abi::*;
use rand::RngCore;
use rand_pcg::Pcg32;
use std::{collections::BinaryHeap, convert::TryFrom, io::Write};

fn interpret_value(repr: u64) -> InterpValue {
    if repr == 0 {
        return InterpValue::Undefined;
    }
    let float = f64::from_bits(repr);
    if float.is_nan() && repr != f64::NAN.to_bits() {
        let id = repr as u32;
        if id == ZERO_ID {
            return InterpValue::Number(0.);
        }
        return InterpValue::Ref(id);
    }
    InterpValue::Number(float)
}

unsafe fn read_value_slice(mut ptr: u64, len: u64) -> Vec<InterpValue> {
    let mut values = Vec::new();
    for _ in 0..len {
        let p = usize::try_from(ptr).expect("Go pointer didn't fit in usize");
        values.push(interpret_value(wavm::caller_load64(p)));
        ptr += 8;
    }
    values
}

#[no_mangle]
pub unsafe extern "C" fn go__debug(x: usize) {
    println!("go debug: {}", x);
}

#[no_mangle]
pub unsafe extern "C" fn go__runtime_resetMemoryDataView(_: usize) {}

/// Safety: λ(code int32)
#[no_mangle]
pub unsafe extern "C" fn go__runtime_wasmExit(sp: usize) {
    let mut sp = GoStack::new(sp);
    std::process::exit(sp.read_u32() as i32);
}

/// Safety: λ(fd uintptr, p pointer, len int32)
pub unsafe extern "C" fn go__runtime_wasmWrite(sp: usize) {
    let mut sp = GoStack::new(sp);
    let fd = sp.read_u64();
    let ptr = sp.read_u64();
    let len = sp.read_u32();
    let buf = wavm::read_slice(ptr, len.into());
    if fd == 2 {
        let stderr = std::io::stderr();
        let mut stderr = stderr.lock();
        stderr.write_all(&buf).unwrap();
    } else {
        let stdout = std::io::stdout();
        let mut stdout = stdout.lock();
        stdout.write_all(&buf).unwrap();
    }
}

// An increasing clock used when Go asks for time, measured in nanoseconds.
static mut TIME: u64 = 0;
// The amount of TIME advanced each check. Currently 10 milliseconds.
static mut TIME_INTERVAL: u64 = 10_000_000;

/// Safety: λ() int64
#[no_mangle]
pub unsafe extern "C" fn go__runtime_nanotime1(sp: usize) {
    let mut sp = GoStack::new(sp);
    TIME += TIME_INTERVAL;
    sp.write_u64(TIME);
}

/// Safety: λ() (seconds int64, nanos int32)
#[no_mangle]
pub unsafe extern "C" fn go__runtime_walltime(sp: usize) {
    let mut sp = GoStack::new(sp);
    TIME += TIME_INTERVAL;
    sp.write_u64(TIME / 1_000_000_000);
    sp.write_u32((TIME % 1_000_000_000) as u32);
}

#[no_mangle]
pub unsafe extern "C" fn go__runtime_walltime1(sp: usize) {
    let mut sp = GoStack::new(sp);
    TIME += TIME_INTERVAL;
    sp.write_u64(TIME / 1_000_000_000);
    sp.write_u64(TIME % 1_000_000_000);
}

static mut RNG: Option<Pcg32> = None;

unsafe fn get_rng<'a>() -> &'a mut Pcg32 {
    RNG.get_or_insert_with(|| Pcg32::new(0xcafef00dd15ea5e5, 0xa02bdbf7bb3c0a7))
}

/// Safety: λ(dest []byte)
#[no_mangle]
pub unsafe extern "C" fn go__runtime_getRandomData(sp: usize) {
    let mut sp = GoStack::new(sp);
    let rng = get_rng();
    let mut ptr = usize::try_from(sp.read_u64()).expect("Go getRandomData pointer not a usize");
    let mut len = sp.read_u64();
    while len >= 4 {
        wavm::caller_store32(ptr, rng.next_u32());
        ptr += 4;
        len -= 4;
    }
    if len > 0 {
        let mut rem = rng.next_u32();
        for _ in 0..len {
            wavm::caller_store8(ptr, rem as u8);
            ptr += 1;
            rem >>= 8;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TimeoutInfo {
    time: u64,
    id: u32,
}

impl Ord for TimeoutInfo {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .time
            .cmp(&self.time)
            .then_with(|| other.id.cmp(&self.id))
    }
}

impl PartialOrd for TimeoutInfo {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(&other))
    }
}

#[derive(Default, Debug)]
struct TimeoutState {
    /// Contains tuples of (time, id)
    times: BinaryHeap<TimeoutInfo>,
    pending_ids: HashSet<u32>,
    next_id: u32,
}

static mut TIMEOUT_STATE: Option<TimeoutState> = None;

/// Safety: λ() (delay int64) int32
#[no_mangle]
pub unsafe extern "C" fn go__runtime_scheduleTimeoutEvent(sp: usize) {
    let mut sp = GoStack::new(sp);
    let mut time = sp.read_u64();
    time = time.saturating_mul(1_000_000); // milliseconds to nanoseconds
    time = time.saturating_add(TIME); // add the current time to the delay

    let state = TIMEOUT_STATE.get_or_insert_with(Default::default);
    let id = state.next_id;
    state.next_id += 1;
    state.times.push(TimeoutInfo { time, id });
    state.pending_ids.insert(id);

    sp.write_u32(id);
}

/// Safety: λ(id int32)
#[no_mangle]
pub unsafe extern "C" fn go__runtime_clearTimeoutEvent(sp: usize) {
    let mut sp = GoStack::new(sp);
    let id = sp.read_u32();

    let state = TIMEOUT_STATE.get_or_insert_with(Default::default);
    if !state.pending_ids.remove(&id) {
        eprintln!("Go attempting to clear not pending timeout event {}", id);
    }
}

macro_rules! unimpl_js {
    ($($f:ident),* $(,)?) => {
        $(
            #[no_mangle]
            pub unsafe extern "C" fn $f(_: usize) {
                unimplemented!("Go JS interface {} not supported", stringify!($f));
            }
        )*
    }
}

unimpl_js!(
    go__syscall_js_stringVal,
    go__syscall_js_valueSetIndex,
    go__syscall_js_valuePrepareString,
    go__syscall_js_valueLoadString,
    go__syscall_js_valueDelete,
    go__syscall_js_valueInvoke,
    go__syscall_js_valueInstanceOf,
);

/// Safety: λ(v value, field string) value
#[no_mangle]
pub unsafe extern "C" fn go__syscall_js_valueGet(sp: usize) {
    let mut sp = GoStack::new(sp);
    let source = interpret_value(sp.read_u64());
    let field = sp.read_js_string();

    let value = match source {
        InterpValue::Ref(id) => get_field(id, &field),
        val => {
            eprintln!(
                "Go attempting to read field {:?} . {}",
                val,
                String::from_utf8_lossy(&field),
            );
            GoValue::Null
        }
    };
    sp.write_u64(value.encode());
}

/// Safety: λ(v value, args []value) (value, bool)
#[no_mangle]
pub unsafe extern "C" fn go__syscall_js_valueNew(sp: usize) {
    let mut sp = GoStack::new(sp);
    let class = sp.read_u32();
    let (args_ptr, args_len) = sp.skip_u32().read_go_slice();
    let args = read_value_slice(args_ptr, args_len);
    if class == UINT8_ARRAY_ID {
        if let Some(InterpValue::Number(size)) = args.get(0) {
            let id = DynamicObjectPool::singleton()
                .insert(DynamicObject::Uint8Array(vec![0; *size as usize]));
            sp.write_u64(GoValue::Object(id).encode());
            sp.write_u8(1);
            return;
        } else {
            eprintln!(
                "Go attempted to construct Uint8Array with bad args: {:?}",
                args,
            );
        }
    } else if class == DATE_ID {
        let id = DynamicObjectPool::singleton().insert(DynamicObject::Date);
        sp.write_u64(GoValue::Object(id).encode());
        sp.write_u8(1);
        return;
    } else {
        eprintln!(
            "Go attempting to construct unimplemented JS value {}",
            class,
        );
    }
    sp.write_u64(GoValue::Null.encode());
    sp.write_u8(0);
}

/// Safety: λ(dest value, src []byte) (int, bool)
#[no_mangle]
pub unsafe extern "C" fn go__syscall_js_copyBytesToJS(sp: usize) {
    let mut sp = GoStack::new(sp);
    let dest_val = interpret_value(sp.read_u64());
    let (src_ptr, src_len) = sp.read_go_slice();

    if let InterpValue::Ref(dest_id) = dest_val {
        let dest = DynamicObjectPool::singleton().get_mut(dest_id);
        if let Some(DynamicObject::Uint8Array(buf)) = dest {
            if buf.len() as u64 != src_len {
                eprintln!(
                    "Go copying bytes from Go source length {} to JS dest length {}",
                    src_len,
                    buf.len(),
                );
            }
            let len = std::cmp::min(src_len, buf.len() as u64) as usize;
            // Slightly inefficient as this allocates a new temporary buffer
            buf[..len].copy_from_slice(&wavm::read_slice(src_ptr, len as u64));
            sp.write_u64(GoValue::Number(len as f64).encode());
            sp.write_u8(1);
            return;
        } else {
            eprintln!(
                "Go attempting to copy bytes into unsupported target {:?}",
                dest,
            );
        }
    } else {
        eprintln!("Go attempting to copy bytes into {:?}", dest_val);
    }
    sp.write_u64(GoValue::Null.encode());
    sp.write_u8(0);
}

/// Safety: λ(dest []byte, src value) (int, bool)
#[no_mangle]
pub unsafe extern "C" fn go__syscall_js_copyBytesToGo(sp: usize) {
    let mut sp = GoStack::new(sp);
    let (dest_ptr, dest_len) = sp.read_go_slice();
    let src_val = interpret_value(sp.read_u64());

    if let InterpValue::Ref(src_id) = src_val {
        let source = DynamicObjectPool::singleton().get_mut(src_id);
        if let Some(DynamicObject::Uint8Array(buf)) = source {
            if buf.len() as u64 != dest_len {
                eprintln!(
                    "Go copying bytes from JS source length {} to Go dest length {}",
                    buf.len(),
                    dest_len,
                );
            }
            let len = std::cmp::min(buf.len() as u64, dest_len) as usize;
            wavm::write_slice(&buf[..len], dest_ptr);

            sp.write_u64(GoValue::Number(len as f64).encode());
            sp.write_u8(1);
            return;
        } else {
            eprintln!(
                "Go attempting to copy bytes from unsupported source {:?}",
                source,
            );
        }
    } else {
        eprintln!("Go attempting to copy bytes from {:?}", src_val);
    }
    sp.skip_u64().write_u8(0);
}

/// Safety: λ(v value, method string, args []value) (value, bool)
unsafe fn value_call_impl(sp: &mut GoStack) -> Result<GoValue, String> {
    let object = interpret_value(sp.read_u64());
    let method_name = sp.read_js_string();
    let (args_ptr, args_len) = sp.read_go_slice();
    let args = read_value_slice(args_ptr, args_len);

    if object == InterpValue::Ref(GO_ID) && &method_name == b"_makeFuncWrapper" {
        let id = args.get(0).ok_or_else(|| {
            format!(
                "Go attempting to call Go._makeFuncWrapper with bad args {:?}",
                args,
            )
        })?;
        let ref_id =
            DynamicObjectPool::singleton().insert(DynamicObject::FunctionWrapper(*id, object));
        Ok(GoValue::Function(ref_id))
    } else if object == InterpValue::Ref(FS_ID) && &method_name == b"write" {
        let args_len = std::cmp::min(6, args.len());
        if let &[InterpValue::Number(fd), InterpValue::Ref(buf_id), InterpValue::Number(offset), InterpValue::Number(length), InterpValue::Ref(NULL_ID), InterpValue::Ref(callback_id)] =
            &args.as_slice()[..args_len]
        {
            let object_pool = DynamicObjectPool::singleton();
            let buf = match object_pool.get(buf_id) {
                Some(DynamicObject::Uint8Array(x)) => x,
                x => {
                    return Err(format!(
                        "Go attempting to call fs.write with bad buffer {:?}",
                        x,
                    ))
                }
            };
            let (func_id, this) = match object_pool.get(callback_id) {
                Some(DynamicObject::FunctionWrapper(f, t)) => (f, t),
                x => {
                    return Err(format!(
                        "Go attempting to call fs.write with bad buffer {:?}",
                        x,
                    ))
                }
            };
            let mut offset = offset as usize;
            let mut length = length as usize;
            if offset > buf.len() {
                eprintln!(
                    "Go attempting to call fs.write with offset {} >= buf.len() {}",
                    offset,
                    buf.len(),
                );
                offset = buf.len();
            }
            if offset + length > buf.len() {
                eprintln!(
                    "Go attempting to call fs.write with offset {} + length {} >= buf.len() {}",
                    offset,
                    length,
                    buf.len(),
                );
                length = buf.len() - offset;
            }

            if fd == 1. {
                let stdout = std::io::stdout();
                let mut stdout = stdout.lock();
                stdout.write_all(&buf[offset..(offset + length)]).unwrap();
            } else if fd == 2. {
                let stderr = std::io::stderr();
                let mut stderr = stderr.lock();
                stderr.write_all(&buf[offset..(offset + length)]).unwrap();
            } else {
                eprintln!("Go attempting to write to unknown FD {}", fd);
            }

            PENDING_EVENT = Some(PendingEvent {
                id: *func_id,
                this: *this,
                args: vec![
                    GoValue::Null,                  // no error
                    GoValue::Number(length as f64), // amount written
                ],
            });

            sp.resume();
            Ok(GoValue::Null)
        } else {
            Err(format!(
                "Go attempting to call fs.write with bad args {:?}",
                args
            ))
        }
    } else if object == InterpValue::Ref(CRYPTO_ID) && &method_name == b"getRandomValues" {
        let id = match args.get(0) {
            Some(InterpValue::Ref(x)) => *x,
            _ => {
                return Err(format!(
                    "Go attempting to call crypto.getRandomValues with bad args {:?}",
                    args,
                ));
            }
        };
        match DynamicObjectPool::singleton().get_mut(id) {
            Some(DynamicObject::Uint8Array(buf)) => {
                get_rng().fill_bytes(buf.as_mut_slice());
            }
            Some(x) => {
                return Err(format!(
                    "Go attempting to call crypto.getRandomValues on bad object {:?}",
                    x,
                ));
            }
            None => {
                return Err(format!(
                    "Go attempting to call crypto.getRandomValues on unknown reference {}",
                    id,
                ));
            }
        }
        Ok(GoValue::Undefined)
    } else if let InterpValue::Ref(obj_id) = object {
        let val = DynamicObjectPool::singleton().get(obj_id);
        if let Some(DynamicObject::Date) = val {
            if &method_name == b"getTimezoneOffset" {
                return Ok(GoValue::Number(0.0));
            } else {
                return Err(format!(
                    "Go attempting to call unknown method {} for date object",
                    String::from_utf8_lossy(&method_name),
                ));
            }
        } else {
            return Err(format!(
                "Go attempting to call method {} for unknown object - id {}",
                String::from_utf8_lossy(&method_name),
                obj_id,
            ));
        }
    } else {
        Err(format!(
            "Go attempting to call unknown method {:?} . {}",
            object,
            String::from_utf8_lossy(&method_name),
        ))
    }
}

/// Safety: λ(v value, method string, args []value) (value, bool)
#[no_mangle]
pub unsafe extern "C" fn go__syscall_js_valueCall(sp: usize) {
    let mut sp = GoStack::new(sp);
    match value_call_impl(&mut sp) {
        Ok(val) => {
            sp.write_u64(val.encode());
            sp.write_u8(1);
        }
        Err(err) => {
            eprintln!("{}", err);
            sp.write_u64(GoValue::Null.encode());
            sp.write_u8(0);
        }
    }
}

/// Safety: λ(v value, field string, x value)
#[no_mangle]
pub unsafe extern "C" fn go__syscall_js_valueSet(sp: usize) {
    let mut sp = GoStack::new(sp);
    let source = interpret_value(sp.read_u64());
    let field = sp.read_js_string();
    let new_value = interpret_value(sp.read_u64());

    if source == InterpValue::Ref(GO_ID)
        && &field == b"_pendingEvent"
        && new_value == InterpValue::Ref(NULL_ID)
    {
        PENDING_EVENT = None;
        return;
    }
    let pool = DynamicObjectPool::singleton();
    if let InterpValue::Ref(id) = source {
        let source = pool.get(id);
        if let Some(DynamicObject::PendingEvent(_)) = source {
            if field == b"result" {
                return;
            }
        }
    }
    eprintln!(
        "Go attempted to set unsupported value {:?} field {} to {:?}",
        source,
        String::from_utf8_lossy(&field),
        new_value,
    );
}

/// Safety: λ(v value) int
#[no_mangle]
pub unsafe extern "C" fn go__syscall_js_valueLength(sp: usize) {
    let mut sp = GoStack::new(sp);
    let source = interpret_value(sp.read_u64());
    let pool = DynamicObjectPool::singleton();
    let source = match source {
        InterpValue::Ref(x) => pool.get(x),
        _ => None,
    };
    let len = match source {
        Some(DynamicObject::Uint8Array(x)) => Some(x.len()),
        Some(DynamicObject::ValueArray(x)) => Some(x.len()),
        _ => None,
    };
    if let Some(len) = len {
        sp.write_u64(len as u64);
    } else {
        eprintln!(
            "Go attempted to get length of unsupported value {:?}",
            source,
        );
        sp.write_u64(0);
    }
}

/// Safety: λ(v value, i int) value
unsafe fn value_index_impl(sp: &mut GoStack) -> Result<GoValue, String> {
    let pool = DynamicObjectPool::singleton();
    let source = match interpret_value(sp.read_u64()) {
        InterpValue::Ref(x) => pool.get(x),
        val => return Err(format!("Go attempted to index into {:?}", val)),
    };
    let index = usize::try_from(sp.read_u64()).map_err(|e| format!("{:?}", e))?;
    let val = match source {
        Some(DynamicObject::Uint8Array(x)) => {
            Some(x.get(index).map(|x| GoValue::Number(*x as f64)))
        }
        Some(DynamicObject::ValueArray(x)) => Some(x.get(index).cloned()),
        _ => None,
    };
    match val {
        Some(Some(val)) => Ok(val),
        Some(None) => Err(format!(
            "Go attempted to index out of bounds into value {:?} index {}",
            source, index,
        )),
        None => Err(format!(
            "Go attempted to index into unsupported value {:?}",
            source
        )),
    }
}

/// Safety: λ(v value, i int) value
#[no_mangle]
pub unsafe extern "C" fn go__syscall_js_valueIndex(sp: usize) {
    let mut sp = GoStack::new(sp);
    match value_index_impl(&mut sp) {
        Ok(v) => sp.write_u64(v.encode()),
        Err(e) => {
            eprintln!("{}", e);
            sp.write_u64(GoValue::Null.encode());
        }
    }
}

/// Safety: λ(v value)
#[no_mangle]
pub unsafe extern "C" fn go__syscall_js_finalizeRef(sp: usize) {
    let mut sp = GoStack::new(sp);
    let val = interpret_value(sp.read_u64());
    match val {
        InterpValue::Ref(x) if x < DYNAMIC_OBJECT_ID_BASE => {}
        InterpValue::Ref(x) => {
            if DynamicObjectPool::singleton().remove(x).is_none() {
                eprintln!("Go attempting to finalize unknown ref {}", x);
            }
        }
        val => eprintln!("Go attempting to finalize {:?}", val),
    }
}

#[no_mangle]
pub unsafe extern "C" fn wavm__go_after_run() {
    let mut state = TIMEOUT_STATE.get_or_insert_with(Default::default);
    while let Some(info) = state.times.pop() {
        while state.pending_ids.contains(&info.id) {
            TIME = std::cmp::max(TIME, info.time);

            #[allow(clippy::drop_ref)]
            drop(state); // wavm_guest_call__resume is re-entrant, so cut the ref's lifetime

            wavm_guest_call__resume();
            state = TIMEOUT_STATE.get_or_insert_with(Default::default);
        }
    }
}
