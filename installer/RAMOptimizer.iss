; Installer for RAM Optimizer, built with Inno Setup 6 by
; .github/workflows/windows.yml:
;   ISCC /DAppVersion=0.4.0 installer\RAMOptimizer.iss
;
; Installs for the current user only (no admin prompt), like VS Code's and
; Discord's installers. That folder stays writable, so the app keeps
; updating itself in place after it's installed.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
; Never change AppId: Windows uses it to recognize upgrades. The app reads
; the matching uninstall entry (see sync_installed_version).
AppId={{10180DC6-8318-4F40-9C1E-DCB8345C55B4}
AppName=RAM Optimizer
AppVersion={#AppVersion}
AppVerName=RAM Optimizer {#AppVersion}
AppPublisher=RAM Optimizer
AppPublisherURL=https://ramoptimizer.is-a.dev/
AppSupportURL=https://github.com/monyratanaksy-byte/ram-optimizer/issues
VersionInfoVersion={#AppVersion}
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\RAM Optimizer
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\target\installer
OutputBaseFilename=RAMOptimizerSetup
SetupIconFile=..\crates\app\assets\icon.ico
UninstallDisplayIcon={app}\RAMOptimizer.exe
UninstallDisplayName=RAM Optimizer
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes

[Tasks]
Name: "desktopicon"; Description: "Add a desktop shortcut"; Flags: unchecked

[Files]
Source: "..\target\release\ram-optimizer.exe"; DestDir: "{app}"; DestName: "RAMOptimizer.exe"; Flags: ignoreversion

[Icons]
; Same AppUserModelID the app sets, so its taskbar button and notifications
; group with the shortcut.
Name: "{autoprograms}\RAM Optimizer"; Filename: "{app}\RAMOptimizer.exe"; AppUserModelID: "RAMOptimizer.App"
Name: "{autodesktop}\RAM Optimizer"; Filename: "{app}\RAMOptimizer.exe"; AppUserModelID: "RAMOptimizer.App"; Tasks: desktopicon

[Run]
Filename: "{app}\RAMOptimizer.exe"; Description: "Open RAM Optimizer"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Quits the running copy (which puts every app it changed back to normal)
; and stops it starting with Windows.
Filename: "{app}\RAMOptimizer.exe"; Parameters: "--uninstall"; Flags: runhidden waituntilterminated; RunOnceId: "QuitAndRemoveAutostart"

[UninstallDelete]
; Leftovers from updates and the log file. Settings in %APPDATA% stay, in
; case it's reinstalled.
Type: filesandordirs; Name: "{app}"

[Code]
// Asks a running copy to quit cleanly (it puts every app it changed back to
// normal) so its file can be replaced and the new one can take over.
function QuitRunningCopy(const Exe: String): Boolean;
var
  ResultCode: Integer;
begin
  Result := True;
  if FileExists(Exe) then
    Result := Exec(Exe, '--quit', '', SW_HIDE, ewWaitUntilTerminated, ResultCode) and (ResultCode = 0);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  // Upgrading: the installed copy may be running.
  QuitRunningCopy(ExpandConstant('{app}\RAMOptimizer.exe'));
  Result := '';
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
begin
  // A copy run straight from Downloads would otherwise keep running and the
  // installed one would just show its window. Versions before 0.4 can't be
  // asked to quit, so those get ended.
  if CurStep = ssPostInstall then
    if not QuitRunningCopy(ExpandConstant('{app}\RAMOptimizer.exe')) then
      Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM RAMOptimizer.exe', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;
