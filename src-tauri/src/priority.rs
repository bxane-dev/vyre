use windows_sys::Win32::{
    Foundation::CloseHandle,
    System::Threading::{
        GetPriorityClass, OpenProcess, SetPriorityClass, ABOVE_NORMAL_PRIORITY_CLASS,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
    },
};

pub fn current(pid: u32) -> Result<u32, String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return Err(format!(
                "Could not open process {pid}: {}",
                std::io::Error::last_os_error()
            ));
        }
        let original = GetPriorityClass(handle);
        if original == 0 {
            let error = std::io::Error::last_os_error();
            CloseHandle(handle);
            return Err(format!("Could not read process priority: {error}"));
        }
        CloseHandle(handle);
        Ok(original)
    }
}

pub fn set_above_normal(pid: u32) -> Result<(), String> {
    unsafe {
        let handle = OpenProcess(PROCESS_SET_INFORMATION, 0, pid);
        if handle.is_null() {
            return Err(format!(
                "Could not open process {pid}: {}",
                std::io::Error::last_os_error()
            ));
        }
        let changed = SetPriorityClass(handle, ABOVE_NORMAL_PRIORITY_CLASS);
        let error = std::io::Error::last_os_error();
        CloseHandle(handle);
        if changed == 0 {
            Err(format!("Could not set process priority: {error}"))
        } else {
            Ok(())
        }
    }
}

pub fn is_above_normal(priority: u32) -> bool {
    priority == windows_sys::Win32::System::Threading::ABOVE_NORMAL_PRIORITY_CLASS
}

pub fn restore(pid: u32, original: u32) -> Result<(), String> {
    unsafe {
        let handle = OpenProcess(PROCESS_SET_INFORMATION, 0, pid);
        if handle.is_null() {
            return Err(format!(
                "Could not open process {pid} for restore: {}",
                std::io::Error::last_os_error()
            ));
        }
        let restored = SetPriorityClass(handle, original);
        let error = std::io::Error::last_os_error();
        CloseHandle(handle);
        if restored == 0 {
            Err(format!("Could not restore process priority: {error}"))
        } else {
            Ok(())
        }
    }
}
