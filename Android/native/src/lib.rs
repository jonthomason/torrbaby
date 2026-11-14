//! The JNI side of Torrbaby away from home for the Android TV app (com.torrbaby.tv.Relay). The
//! work is tbdirect's proxy mode (Apple/Direct/src/proxy.rs), on the code the iPhone uses: one
//! loopback port whose every connection goes to Diamond by a punched direct path when one is
//! open, else as a stream bronze's relay splices to Diamond (TLS end to end to Diamond's pinned
//! identity once one is pinned), and a maintainer that keeps those paths.
use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jint, jstring, JNI_FALSE, JNI_TRUE};
use std::ffi::CString;

fn text(env: &mut JNIEnv, value: &JString) -> Option<String> {
    if value.is_null() { return None }
    env.get_string(value).ok().map(Into::into)
}

fn string(env: &mut JNIEnv, value: &JString) -> Option<CString> { CString::new(text(env, value)?).ok() }

/// Start the proxy (tbdirect::proxy): one loopback port whose every connection goes to Diamond
/// by the direct path when one is punched, else the relay to bronze at `address` ("ip:port"),
/// pinned to `fingerprint`. `token` is this TV's token, which punching asks Diamond with
/// (empty: relay only); `stateDir` keeps the carriers where punching has worked. Returns the
/// loopback port, or 0 when the relay did not open. Already running for this relay, it takes
/// the new token and returns at once; otherwise it blocks for up to 7 s: call off the main thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_torrbaby_tv_Relay_nativeStart(mut env: JNIEnv, _: JClass, address: JString, fingerprint: JString, token: JString, state_dir: JString) -> jint {
    let (Some(address), Some(fingerprint)) = (text(&mut env, &address), text(&mut env, &fingerprint)) else { return 0 };
    tbdirect::proxy::set_logger(log);
    let mut config = tbdirect::proxy::Config::new(&address, &fingerprint);
    config.token = text(&mut env, &token).unwrap_or_default();
    config.state_dir = text(&mut env, &state_dir).filter(|d| !d.is_empty()).map(std::path::PathBuf::from);
    tbdirect::proxy::start(config) as jint
}

/// Whether the proxy is running (it keeps its own paths open, or wakes them when needed).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_torrbaby_tv_Relay_nativeAlive(_: JNIEnv, _: JClass) -> jboolean {
    if tbdirect::proxy::running() { JNI_TRUE } else { JNI_FALSE }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_torrbaby_tv_Relay_nativeStop(_: JNIEnv, _: JClass) { tbdirect::proxy::stop() }

/// The default network changed (from ConnectivityManager): `signature` identifies it, `interface`
/// is its name. A change that holds for 3 s drops the direct path and moves the relay.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_torrbaby_tv_Relay_nativeNetwork(mut env: JNIEnv, _: JClass, signature: JString, interface: JString) {
    let signature = text(&mut env, &signature).unwrap_or_default();
    let interface = text(&mut env, &interface).unwrap_or_default();
    // Usually told before the proxy first starts: its line goes to logcat too.
    tbdirect::proxy::set_logger(log);
    tbdirect::proxy::network(&signature, &interface);
}

/// One line on the proxy's paths, the last punch and what it carried, for logcat.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_torrbaby_tv_Relay_nativeStatus<'local>(env: JNIEnv<'local>, _: JClass<'local>) -> jstring {
    env.new_string(tbdirect::proxy::status()).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}

/// Each step the proxy's maintainer takes goes to logcat (tag TorrbabyDirect), as it happens.
fn log(line: &str) {
    #[cfg(target_os = "android")]
    {
        #[link(name = "log")]
        unsafe extern "C" { fn __android_log_write(priority: std::ffi::c_int, tag: *const std::ffi::c_char, text: *const std::ffi::c_char) -> std::ffi::c_int; }
        if let Ok(line) = CString::new(line) { unsafe { __android_log_write(4, c"TorrbabyDirect".as_ptr(), line.as_ptr()); } }
    }
    #[cfg(not(target_os = "android"))]
    eprintln!("{line}");
}

/// Diamond's identity fingerprint (64 hex): relayed streams are then encrypted end to end to
/// it. Empty forgets it.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_torrbaby_tv_Relay_nativeIdentity(mut env: JNIEnv, _: JClass, fingerprint: JString) {
    let fingerprint = string(&mut env, &fingerprint).unwrap_or_default();
    tbdirect::tbdirect_identity(fingerprint.as_ptr());
}

/// This TV's client id, naming its relay connection to bronze.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_torrbaby_tv_Relay_nativeClient(mut env: JNIEnv, _: JClass, id: JString) {
    let id = string(&mut env, &id).unwrap_or_default();
    tbdirect::tbdirect_client(id.as_ptr());
}
