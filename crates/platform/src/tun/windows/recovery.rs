//! The elevated helper records physical interface state before the core can
//! change it. A fresh helper can recover this record after forced termination.
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use windows_sys::Win32::{
    NetworkManagement::{
        IpHelper::{
            ConvertInterfaceGuidToLuid, ConvertInterfaceLuidToGuid, FreeMibTable,
            GetIpInterfaceEntry, GetIpInterfaceTable, InitializeIpInterfaceEntry,
            MIB_IPINTERFACE_ROW, MIB_IPINTERFACE_TABLE, SetIpInterfaceEntry,
        },
        Ndis::NET_LUID_LH,
    },
    Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC},
};
use windows_sys::core::GUID;

const JOURNAL: &str = r"C:\Program Files\v2rayR\windows-tun\run\network-v2.json";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Interface {
    guid: (u32, u16, u16, [u8; 8]),
    family: u16,
}

fn status(code: u32) -> io::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}

pub fn recovery_pending() -> io::Result<bool> {
    match std::fs::symlink_metadata(JOURNAL) {
        Ok(_) if super::regular(JOURNAL, false) => Ok(true),
        Ok(_) => Err(super::invalid("Windows TUN 恢复记录不是普通文件")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn capture_network() -> io::Result<()> {
    let mut table = std::ptr::null_mut();
    // SAFETY: the OS populates the pointer and owns the table allocation.
    status(unsafe { GetIpInterfaceTable(AF_UNSPEC, &mut table) })?;
    struct Table(*mut MIB_IPINTERFACE_TABLE);
    impl Drop for Table {
        fn drop(&mut self) {
            unsafe { FreeMibTable(self.0.cast()) };
        }
    }
    let table = Table(table);
    let mut interfaces = Vec::new();
    // SAFETY: GetIpInterfaceTable returns NumEntries contiguous rows at Table.
    let rows = unsafe {
        std::slice::from_raw_parts(
            std::ptr::addr_of!((*table.0).Table).cast::<MIB_IPINTERFACE_ROW>(),
            (*table.0).NumEntries as usize,
        )
    };
    for row in rows.iter().filter(|row| row.WeakHostSend) {
        let mut guid = GUID::default();
        status(unsafe { ConvertInterfaceLuidToGuid(&row.InterfaceLuid, &mut guid) })?;
        interfaces.push(Interface {
            guid: (guid.data1, guid.data2, guid.data3, guid.data4),
            family: row.Family,
        });
    }

    let mut file = tempfile::NamedTempFile::new_in(super::RUNTIME)?;
    serde_json::to_writer(&mut file, &interfaces).map_err(io::Error::other)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(JOURNAL)
        .map_err(|error| error.error)?;
    Ok(())
}

pub fn restore_network() -> io::Result<()> {
    if !recovery_pending()? {
        return Ok(());
    }
    if !super::network_cleared() && !super::clear_owned_adapter() {
        return Err(super::invalid("Windows TUN 网卡或路由仍在使用"));
    }
    let entries: Vec<Interface> =
        serde_json::from_reader(std::fs::File::open(JOURNAL)?).map_err(io::Error::other)?;
    let mut result = Ok(());
    for entry in entries {
        // Try all interfaces even when one has been detached. Keep the record
        // until every change can be verified, including on a subsequent retry.
        if let Err(error) = restore_interface(entry) {
            result = Err(error);
        }
    }
    result?;
    std::fs::remove_file(Path::new(JOURNAL))
}

fn restore_interface(entry: Interface) -> io::Result<()> {
    if !matches!(entry.family, AF_INET | AF_INET6) {
        return Err(super::invalid("无效的 Windows TUN 接口地址族"));
    }
    let (data1, data2, data3, data4) = entry.guid;
    let guid = GUID {
        data1,
        data2,
        data3,
        data4,
    };
    let mut luid = NET_LUID_LH::default();
    status(unsafe { ConvertInterfaceGuidToLuid(&guid, &mut luid) })?;
    let mut row = MIB_IPINTERFACE_ROW {
        Family: entry.family,
        InterfaceLuid: luid,
        ..Default::default()
    };
    status(unsafe { GetIpInterfaceEntry(&mut row) })?;
    if !row.WeakHostSend {
        // Use the same single-field update as sing-tun. The initializer sets
        // the sentinel values for fields that SetIpInterfaceEntry must ignore.
        unsafe { InitializeIpInterfaceEntry(&mut row) };
        row.Family = entry.family;
        row.InterfaceLuid = luid;
        row.WeakHostSend = true;
        status(unsafe { SetIpInterfaceEntry(&mut row) })?;
        status(unsafe { GetIpInterfaceEntry(&mut row) })?;
    }
    if row.WeakHostSend {
        Ok(())
    } else {
        Err(super::invalid(
            "Windows TUN WeakHostSend 恢复未通过读回检查",
        ))
    }
}
