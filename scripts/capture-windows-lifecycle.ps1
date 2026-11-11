# Read-only evidence capture. Run in the same Windows account as the desktop app.
param(
    [Parameter(Mandatory=$true)][string]$EvidenceDir,
    [Parameter(Mandatory=$true)][string]$TestRoot
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $EvidenceDir | Out-Null
if (-not ('V2rayrEvidence.WinInet' -as [type])) {
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
namespace V2rayrEvidence {
  public static class WinInet {
    [StructLayout(LayoutKind.Explicit)] struct Value {
      [FieldOffset(0)] public uint Number;
      [FieldOffset(0)] public IntPtr Text;
      [FieldOffset(0)] public long FileTime;
    }
    [StructLayout(LayoutKind.Sequential)] struct Option { public uint Id; public Value Data; }
    [StructLayout(LayoutKind.Sequential)] struct List {
      public uint Size; public IntPtr Connection; public uint Count; public uint Error; public IntPtr Options;
    }
    [DllImport("wininet.dll", SetLastError=true)]
    static extern bool InternetQueryOptionW(IntPtr handle, uint option, ref List list, ref uint length);
    [DllImport("kernel32.dll")] static extern IntPtr GlobalFree(IntPtr memory);
    public static string Read(uint id) {
      Option option = new Option(); option.Id = id;
      IntPtr memory = Marshal.AllocHGlobal(Marshal.SizeOf(typeof(Option)));
      try {
        Marshal.StructureToPtr(option, memory, false);
        List list = new List(); list.Size = (uint)Marshal.SizeOf(typeof(List));
        list.Count = 1; list.Options = memory; uint size = list.Size;
        if (!InternetQueryOptionW(IntPtr.Zero, 75, ref list, ref size))
          throw new Win32Exception(Marshal.GetLastWin32Error());
        option = (Option)Marshal.PtrToStructure(memory, typeof(Option));
        if (id == 1) return option.Data.Number.ToString();
        if (option.Data.Text == IntPtr.Zero) return "";
        try { return Marshal.PtrToStringUni(option.Data.Text); }
        finally { GlobalFree(option.Data.Text); }
      } finally { Marshal.FreeHGlobal(memory); }
    }
  }
}
'@
}
@{
    capturedAt = (Get-Date).ToString('o')
    account = [Security.Principal.WindowsIdentity]::GetCurrent().Name
    flags = [V2rayrEvidence.WinInet]::Read(1)
    server = [V2rayrEvidence.WinInet]::Read(2)
    bypass = [V2rayrEvidence.WinInet]::Read(3)
    pac = [V2rayrEvidence.WinInet]::Read(4)
} | ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $EvidenceDir 'wininet.json')
Get-CimInstance Win32_Process |
    Where-Object { $_.Name -match 'v2ray|xray|sing-box|mihomo|msedgewebview2' } |
    Select-Object ProcessId,ParentProcessId,Name,ExecutablePath,CreationDate,CommandLine |
    ConvertTo-Json -Depth 4 | Set-Content -Encoding UTF8 (Join-Path $EvidenceDir 'processes.json')
Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue |
    Select-Object LocalAddress,LocalPort,OwningProcess |
    ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $EvidenceDir 'listeners.json')
Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings' |
    Select-Object ProxyEnable,ProxyServer,ProxyOverride,AutoConfigURL |
    ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $EvidenceDir 'registry-proxy.json')
$journal = Join-Path $TestRoot 'run\takeover.json'
@{ exists = (Test-Path -LiteralPath $journal); root = $TestRoot } |
    ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $EvidenceDir 'journal-state.json')
if (Test-Path -LiteralPath $journal) {
    Copy-Item -LiteralPath $journal -Destination (Join-Path $EvidenceDir 'takeover.json')
    Get-FileHash -Algorithm SHA256 -LiteralPath $journal |
        ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $EvidenceDir 'journal-hash.json')
}
Write-Host "Evidence captured: $EvidenceDir"
