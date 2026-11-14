//! The JNI side of Torrbaby's relay for the Android TV app (com.torrbaby.tv.Relay). The relay
//! itself is tbdirect's, the same code the iPhone uses: QUIC to bronze, a loopback listener
//! whose every connection becomes a stream bronze splices to Diamond, wrapped in TLS to
//! Diamond's pinned identity once one is pinned.
use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jint, JNI_FALSE, JNI_TRUE};
use std::ffi::CString;
use std::os::fd::IntoRawFd;

fn string(env: &mut JNIEnv, value: &JString) -> Option<CString> {
    if value.is_null() { return None }
    let text: String = env.get_string(value).ok()?.into();
    CString::new(text).ok()
}

/// Open the relay to bronze at `address` ("ip:port"), pinned to `fingerprint`, on a fresh UDP
/// socket. Returns the loopback port, or 0. Blocks for up to 5 s: call off the main thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_torrbaby_tv_Relay_nativeStart(mut env: JNIEnv, _: JClass, address: JString, fingerprint: JString) -> jint {
    let (Some(address), Some(fingerprint)) = (string(&mut env, &address), string(&mut env, &fingerprint)) else { return 0 };
    let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") else { return 0 };
    tbdirect::tbdirect_relay_start(socket.into_raw_fd(), address.as_ptr(), fingerprint.as_ptr()) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_torrbaby_tv_Relay_nativeAlive(_: JNIEnv, _: JClass) -> jboolean {
    if tbdirect::tbdirect_relay_alive() { JNI_TRUE } else { JNI_FALSE }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_torrbaby_tv_Relay_nativeStop(_: JNIEnv, _: JClass) { tbdirect::tbdirect_relay_stop() }

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
