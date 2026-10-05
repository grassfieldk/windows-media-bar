#ifndef AppVersion
  #error AppVersion must be supplied by the packaging script
#endif
#ifndef BinaryPath
  #error BinaryPath must be supplied by the packaging script
#endif
#ifndef OutputPath
  #error OutputPath must be supplied by the packaging script
#endif

[Setup]
AppId={{AF445950-A834-46FD-A047-55B77B47E0AF}
AppName=Windows Media Bar
AppVersion={#AppVersion}
AppPublisher=grassfieldk
AppPublisherURL=https://github.com/grassfieldk/windows-media-bar
DefaultDirName={localappdata}\Programs\Windows Media Bar
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
OutputDir={#OutputPath}
OutputBaseFilename=windows-media-bar-v{#AppVersion}-x86_64-setup
UninstallDisplayIcon={app}\windows-media-bar.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"

[CustomMessages]
english.StartupTask=Start Windows Media Bar when you sign in
japanese.StartupTask=Windows へのサインイン時に Windows Media Bar を起動する
english.AutoUpdateTask=Automatically update Windows Media Bar on launch
japanese.AutoUpdateTask=Windows Media Bar の起動時に自動更新する

[Tasks]
Name: "startup"; Description: "{cm:StartupTask}"; Flags: unchecked
Name: "autoupdate"; Description: "{cm:AutoUpdateTask}"; Flags: unchecked

[Files]
Source: "{#BinaryPath}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Windows Media Bar"; Filename: "{app}\windows-media-bar.exe"

[Registry]
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "MediaBar"; ValueData: """{app}\windows-media-bar.exe"""; Tasks: startup; Check: not IsUpdateInstall
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "MediaBar"; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\MediaBar"; ValueType: dword; ValueName: "AutoUpdate"; ValueData: "1"; Tasks: autoupdate; Check: not IsUpdateInstall
Root: HKCU; Subkey: "Software\MediaBar"; ValueType: dword; ValueName: "AutoUpdate"; ValueData: "0"; Check: (not IsUpdateInstall) and (not WizardIsTaskSelected('autoupdate'))

[Run]
Filename: "{app}\windows-media-bar.exe"; Description: "{cm:LaunchProgram,Windows Media Bar}"; Flags: nowait postinstall skipifsilent
Filename: "{app}\windows-media-bar.exe"; Flags: nowait; Check: IsUpdateInstall

[Code]
function IsUpdateInstall(): Boolean;
begin
  Result := ExpandConstant('{param:UPDATE|0}') = '1';
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Window: HWND;
  Attempt: Integer;
begin
  if CurUninstallStep <> usUninstall then
    Exit;
  Window := FindWindowByClassName('MediaBarClass');
  if Window <> 0 then
  begin
    PostMessage(Window, $0010, 0, 0);
    for Attempt := 1 to 50 do
    begin
      if FindWindowByClassName('MediaBarClass') = 0 then
        Break;
      Sleep(100);
    end;
  end;
end;
