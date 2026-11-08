//! The native Kira plugin service ABI.

use crate::net::bytes;

#[cfg(unix)]
use klf_helper_apps::services;

#[cfg(not(unix))]
const UNAVAILABLE: i64 = -1;
const NOT_INSTALLED: i64 = -2;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn klfsvc_plugin_start(id: *const u8, length: i64) -> i64 {
    let Some(name) = bytes(id, length).and_then(|bytes| std::str::from_utf8(bytes).ok()) else {
        return NOT_INSTALLED;
    };
    #[cfg(unix)]
    {
        return services::start(name);
    }
    #[cfg(not(unix))]
    {
        let _ = name;
        UNAVAILABLE
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_plugin_poll(handle: i64) -> i64 {
    #[cfg(unix)]
    {
        return services::poll(handle);
    }
    #[cfg(not(unix))]
    {
        let _ = handle;
        UNAVAILABLE
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_plugin_wait(handle: i64) -> i64 {
    #[cfg(unix)]
    {
        return services::wait(handle);
    }
    #[cfg(not(unix))]
    {
        let _ = handle;
        UNAVAILABLE
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_plugin_stop(handle: i64) -> i64 {
    #[cfg(unix)]
    {
        return services::stop(handle);
    }
    #[cfg(not(unix))]
    {
        let _ = handle;
        UNAVAILABLE
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn klfsvc_plugin_share_file(handle: i64, resource_id: i64, file_handle: i64) -> i64 {
    #[cfg(unix)]
    {
        return services::share_file(handle, resource_id, file_handle);
    }
    #[cfg(not(unix))]
    {
        let _ = (handle, resource_id, file_handle);
        UNAVAILABLE
    }
}
