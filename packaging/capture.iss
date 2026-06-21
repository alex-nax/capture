; Inno Setup script for the Capture Windows installer (feature #34).
; Produces a per-user CaptureSetup-<version>-x64.exe (no admin/UAC by default).
; Driven by packaging/build_windows.ps1, which stages the install tree and passes the
; version/paths via /D defines. See docs/specs/windows-release.md.
;
; Stage tree (StageDir) laid out by build_windows.ps1 (v3, all-Rust):
;   Capture.exe                  the tray agent (entry point + logon task)
;   capture-gui.exe              the GPUI window (CAPTURE_AGENT=1)
;   capture-mcp.exe              the MCP stdio server (the capture tool surface)
;   captured\captured.exe        the Rust daemon (WASAPI/GDI in-process; no PyInstaller, no helper exe)
;   skill\                       the bundled capture skill
;   register_logon_task.ps1      logon-task registrar (run post-install)

#ifndef MyAppVersion
  #define MyAppVersion "0.0.0"
#endif
#ifndef StageDir
  #define StageDir "..\dist\Capture"
#endif
#ifndef OutDir
  #define OutDir "..\dist"
#endif

[Setup]
AppId={{6F3E2A9C-1B4D-4E7A-8C2F-0A1B2C3D4E5F}
AppName=Capture
AppVersion={#MyAppVersion}
AppPublisher=capture-mcp
AppPublisherURL=https://github.com/alex-nax/capture
DefaultDirName={localappdata}\Programs\Capture
DisableProgramGroupPage=yes
; Per-user install - no elevation. (A signed per-machine build can raise this later.)
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputDir={#OutDir}
OutputBaseFilename=CaptureSetup-{#MyAppVersion}-x64
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayName=Capture {#MyAppVersion}
UninstallDisplayIcon={app}\Capture.exe

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs ignoreversion

[Tasks]
Name: "logontask"; Description: "Start Capture automatically at sign-in (runs the tray agent)"; GroupDescription: "Startup:"
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Icons]
Name: "{autoprograms}\Capture"; Filename: "{app}\Capture.exe"
Name: "{autodesktop}\Capture"; Filename: "{app}\Capture.exe"; Tasks: desktopicon

[Run]
; Register the interactive logon task (runs as the installing user; no admin needed).
Filename: "powershell.exe"; \
  Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\register_logon_task.ps1"" -Action register -AgentPath ""{app}\Capture.exe"""; \
  Flags: runhidden; Tasks: logontask
; Offer to launch at the end of an interactive install.
Filename: "{app}\Capture.exe"; Description: "Launch Capture now"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Remove the logon task on uninstall.
Filename: "powershell.exe"; \
  Parameters: "-NoProfile -ExecutionPolicy Bypass -File ""{app}\register_logon_task.ps1"" -Action unregister"; \
  Flags: runhidden; RunOnceId: "UnregLogonTask"

[UninstallDelete]
Type: filesandordirs; Name: "{app}"
