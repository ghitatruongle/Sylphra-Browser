#define MyAppName "Sylphra"
#ifndef MyAppVersion
  #error "Compile via packaging/package.ps1 (or pass /DMyAppVersion=x.y.z)"
#endif
#define MyAppPublisher "Sylphra contributors"
#define MyAppURL "https://github.com/ghitatruongle/Sylphra-Browser"
#define MyAppExeName "sylphra.exe"
#define VersionHyphen Pos("-", MyAppVersion)
#if VersionHyphen > 0
  #define VersionNumeric Copy(MyAppVersion, 1, VersionHyphen - 1)
#else
  #define VersionNumeric MyAppVersion
#endif

[Setup]
AppId={{63F48D80-D9E7-49E4-9FF8-5714790D93A7}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}/issues
AppUpdatesURL={#MyAppURL}/releases
VersionInfoVersion={#VersionNumeric}.0
VersionInfoProductVersion={#VersionNumeric}.0
VersionInfoProductName={#MyAppName}
VersionInfoDescription={#MyAppName} installer
VersionInfoCompany={#MyAppPublisher}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
OutputDir=..\dist
OutputBaseFilename=Sylphra-v{#MyAppVersion}-Setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
LicenseFile=..\LICENSE
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\sylphra.exe"; DestDir: "{app}"; DestName: "{#MyAppExeName}"; Flags: ignoreversion
Source: "..\target\release\sylphra-renderer-worker.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\sylphra-browser-child.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\icon.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\icon.ico"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\icon.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent

[Registry]
Root: HKLM; Subkey: "Software\{#MyAppName}"; ValueType: string; ValueName: "InstallPath"; ValueData: "{app}"; Flags: uninsdeletekey; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\{#MyAppName}"; ValueType: string; ValueName: "Version"; ValueData: "{#MyAppVersion}"; Flags: uninsdeletekey; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\Sylphra.Document"; ValueType: string; ValueName: ""; ValueData: "Sylphra document"; Flags: uninsdeletekey; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\Sylphra.Document\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#MyAppExeName},0"; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\Sylphra.Document\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" ""%1"""; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.html\OpenWithProgids"; ValueType: string; ValueName: "Sylphra.Document"; ValueData: ""; Flags: uninsdeletevalue; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.htm\OpenWithProgids"; ValueType: string; ValueName: "Sylphra.Document"; ValueData: ""; Flags: uninsdeletevalue; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.xhtml\OpenWithProgids"; ValueType: string; ValueName: "Sylphra.Document"; ValueData: ""; Flags: uninsdeletevalue; Check: IsAdminInstallMode
Root: HKLM; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: string; ValueName: "Sylphra.Document"; ValueData: ""; Flags: uninsdeletevalue; Check: IsAdminInstallMode
Root: HKCU; Subkey: "Software\{#MyAppName}"; ValueType: string; ValueName: "InstallPath"; ValueData: "{app}"; Flags: uninsdeletekey; Check: not IsAdminInstallMode
Root: HKCU; Subkey: "Software\{#MyAppName}"; ValueType: string; ValueName: "Version"; ValueData: "{#MyAppVersion}"; Flags: uninsdeletekey; Check: not IsAdminInstallMode
Root: HKCU; Subkey: "Software\Classes\Sylphra.Document"; ValueType: string; ValueName: ""; ValueData: "Sylphra document"; Flags: uninsdeletekey; Check: not IsAdminInstallMode
Root: HKCU; Subkey: "Software\Classes\Sylphra.Document\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#MyAppExeName},0"; Check: not IsAdminInstallMode
Root: HKCU; Subkey: "Software\Classes\Sylphra.Document\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#MyAppExeName}"" ""%1"""; Check: not IsAdminInstallMode
Root: HKCU; Subkey: "Software\Classes\.html\OpenWithProgids"; ValueType: string; ValueName: "Sylphra.Document"; ValueData: ""; Flags: uninsdeletevalue; Check: not IsAdminInstallMode
Root: HKCU; Subkey: "Software\Classes\.htm\OpenWithProgids"; ValueType: string; ValueName: "Sylphra.Document"; ValueData: ""; Flags: uninsdeletevalue; Check: not IsAdminInstallMode
Root: HKCU; Subkey: "Software\Classes\.xhtml\OpenWithProgids"; ValueType: string; ValueName: "Sylphra.Document"; ValueData: ""; Flags: uninsdeletevalue; Check: not IsAdminInstallMode
Root: HKCU; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: string; ValueName: "Sylphra.Document"; ValueData: ""; Flags: uninsdeletevalue; Check: not IsAdminInstallMode

[Code]
const
  LegacyProductList = 'Vernayf,GhitaBrowser';
  LegacyFolderList = 'Vernayf,GhitaBrowser';
  LegacyDataList = 'Vernayf,GhitaBrowser,.vernayf_data,.ghitabrowser_data';
  LegacyProgIdList = 'Vernayf.Document,GhitaBrowser.Document,GhitaBrowser.HTMLDocument';
  LegacyExtensionList = '.html,.htm,.xhtml,.pdf';
  LegacyRootList = 'HKCU,HKLM,HKLM32';
  LegacyClassSuffixList = ',.Document,.HTMLDocument';
  UninstallerList = 'unins000.exe,unins001.exe,unins002.exe,uninstall.exe,uninstaller.exe';

function RootKeyAt(const Index: Integer): String;
begin
  case Index of
    0: Result := 'HKCU';
    1: Result := 'HKLM';
    2: Result := 'HKLM32';
  else
    Result := '';
  end;
end;

procedure LogLine(const Report: TStringList; const Line: String);
begin
  if Trim(Line) <> '' then
    Report.Add(Line);
end;

function DeleteTreeIfPresent(const Path: String): Boolean;
begin
  Result := DirExists(Path) and DelTree(Path, True, True, True);
end;

function BuildProgramRoots: TStringList;
var
  LetterIndex: Integer;
  Drive: String;
begin
  Result := TStringList.Create;
  Result.Add(ExpandConstant('{pf}'));
  Result.Add(ExpandConstant('{pf32}'));
  Result.Add(ExpandConstant('{commonpf}'));
  Result.Add(ExpandConstant('{commonpf32}'));
  for LetterIndex := 0 to 25 do
  begin
    Drive := Chr(Ord('A') + LetterIndex) + ':\';
    if not DirExists(Drive) then
      Continue;
    Result.Add(Drive + 'Program Files');
    Result.Add(Drive + 'Program Files (x86)');
    Result.Add(Drive + 'ProgramData');
    Result.Add(Drive + 'Users');
    Result.Add(Drive);
  end;
end;

function BuildDataRoots: TStringList;
begin
  Result := TStringList.Create;
  Result.Add(ExpandConstant('{localappdata}'));
  Result.Add(ExpandConstant('{localappdata}') + '\Programs');
  Result.Add(ExpandConstant('{userappdata}'));
  Result.Add(ExpandConstant('{commonappdata}'));
  Result.Add(ExpandConstant('{userdocs}'));
end;

function BuildShortcutRoots: TStringList;
begin
  Result := TStringList.Create;
  Result.Add(ExpandConstant('{userprograms}'));
  Result.Add(ExpandConstant('{commonprograms}'));
  Result.Add(ExpandConstant('{userdesktop}'));
  Result.Add(ExpandConstant('{commondesktop}'));
  Result.Add(ExpandConstant('{userstartmenu}'));
  Result.Add(ExpandConstant('{commonstartmenu}'));
  Result.Add(ExpandConstant('{userstartup}'));
  Result.Add(ExpandConstant('{commonstartup}'));
end;

procedure RunUninstallerInside(const Folder: String; const Report: TStringList);
var
  Candidates: TStringList;
  Index: Integer;
  Path: String;
  ResultCode: Integer;
  RanAny: Boolean;
begin
  Candidates := TStringList.Create;
  try
    Candidates.CommaText := UninstallerList;
    RanAny := False;
    for Index := 0 to Candidates.Count - 1 do
    begin
      if RanAny then
        Continue;
      Path := Folder + '\' + Trim(Candidates[Index]);
      if not FileExists(Path) then
        Continue;
      if Exec('"' + Path + '"', '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART', '',
        SW_HIDE, ewWaitUntilTerminated, ResultCode) then
        LogLine(Report, 'Ran legacy uninstaller: ' + Path)
      else
        LogLine(Report, 'Legacy uninstaller could not run, deleting folder instead: ' + Path);
      RanAny := True;
    end;
  finally
    Candidates.Free;
  end;
end;

function SweepFolders(const Names: String; const Roots: TStringList;
  const Report: TStringList): Boolean;
var
  NameList: TStringList;
  NameIndex: Integer;
  RootIndex: Integer;
  Candidate: String;
  Found: Boolean;
begin
  Result := False;
  NameList := TStringList.Create;
  try
    NameList.CommaText := Names;
    for NameIndex := 0 to NameList.Count - 1 do
      for RootIndex := 0 to Roots.Count - 1 do
      begin
        Candidate := Roots[RootIndex] + '\' + Trim(NameList[NameIndex]);
        if not DirExists(Candidate) then
          Continue;
        RunUninstallerInside(Candidate, Report);
        if DeleteTreeIfPresent(Candidate) then
        begin
          LogLine(Report, 'Removed folder: ' + Candidate);
          Found := True;
        end
        else
          LogLine(Report, 'COULD NOT REMOVE: ' + Candidate);
      end;
    Result := Found;
  finally
    NameList.Free;
  end;
end;

function SweepShortcuts(const Names: String; const Roots: TStringList;
  const Report: TStringList): Boolean;
var
  NameList: TStringList;
  NameIndex: Integer;
  RootIndex: Integer;
  Link: String;
  Found: Boolean;
begin
  Result := False;
  NameList := TStringList.Create;
  try
    NameList.CommaText := Names;
    for NameIndex := 0 to NameList.Count - 1 do
      for RootIndex := 0 to Roots.Count - 1 do
      begin
        Link := Roots[RootIndex] + '\' + Trim(NameList[NameIndex]) + '.lnk';
        if FileExists(Link) and DeleteFile(Link) then
        begin
          LogLine(Report, 'Removed shortcut: ' + Link);
          Found := True;
        end;
      end;
    Result := Found;
  finally
    NameList.Free;
  end;
end;

procedure DeleteKeyWithRegTool(const RootKey: String; const SubKey: String;
  const Report: TStringList);
var
  ResultCode: Integer;
  FullKey: String;
begin
  FullKey := RootKey + '\' + SubKey;
  if not Exec(ExpandConstant('{sys}\reg.exe'), 'delete "' + FullKey + '" /f',
    '', SW_HIDE, ewWaitUntilTerminated, ResultCode) then
    Exit;
  if ResultCode = 0 then
    LogLine(Report, 'Removed registry key: ' + FullKey);
end;

procedure DeleteValueWithRegTool(const RootKey: String; const SubKey: String;
  const ValueName: String; const Report: TStringList);
var
  ResultCode: Integer;
  FullKey: String;
begin
  FullKey := RootKey + '\' + SubKey;
  if not Exec(ExpandConstant('{sys}\reg.exe'),
    'delete "' + FullKey + '" /v "' + ValueName + '" /f', '',
    SW_HIDE, ewWaitUntilTerminated, ResultCode) then
    Exit;
  if ResultCode = 0 then
    LogLine(Report, 'Removed registry value: ' + FullKey + ' :: ' + ValueName);
end;

function SweepRegistry(const Names: String; const Report: TStringList): Boolean;
var
  NameList: TStringList;
  SuffixList: TStringList;
  RootList: TStringList;
  NameIndex: Integer;
  SuffixIndex: Integer;
  RootIndex: Integer;
  Name: String;
  Suffix: String;
  RootKey: String;
  Found: Boolean;
begin
  Result := False;
  NameList := TStringList.Create;
  SuffixList := TStringList.Create;
  RootList := TStringList.Create;
  try
    NameList.CommaText := Names;
    SuffixList.CommaText := LegacyClassSuffixList;
    RootList.CommaText := LegacyRootList;
    for NameIndex := 0 to NameList.Count - 1 do
    begin
      Name := Trim(NameList[NameIndex]);
      for RootIndex := 0 to RootList.Count - 1 do
      begin
        RootKey := RootKeyAt(RootIndex);
        for SuffixIndex := 0 to SuffixList.Count - 1 do
        begin
          Suffix := Trim(SuffixList[SuffixIndex]);
          DeleteKeyWithRegTool(RootKey, 'Software\Classes\' + Name + Suffix, Report);
          Found := True;
        end;
        DeleteKeyWithRegTool(RootKey, 'Software\' + Name, Report);
        Found := True;
      end;
    end;
    Result := Found;
  finally
    NameList.Free;
    SuffixList.Free;
    RootList.Free;
  end;
end;

function SweepAssociations(const ProgIds: String; const Report: TStringList): Boolean;
var
  IdList: TStringList;
  RootList: TStringList;
  ExtList: TStringList;
  IdIndex: Integer;
  RootIndex: Integer;
  ExtIndex: Integer;
  ProgId: String;
  RootKey: String;
  Extension: String;
  Found: Boolean;
begin
  Result := False;
  IdList := TStringList.Create;
  RootList := TStringList.Create;
  ExtList := TStringList.Create;
  try
    IdList.CommaText := ProgIds;
    RootList.CommaText := LegacyRootList;
    ExtList.CommaText := LegacyExtensionList;
    for IdIndex := 0 to IdList.Count - 1 do
    begin
      ProgId := Trim(IdList[IdIndex]);
      for RootIndex := 0 to RootList.Count - 1 do
      begin
        RootKey := RootKeyAt(RootIndex);
        for ExtIndex := 0 to ExtList.Count - 1 do
        begin
          Extension := Trim(ExtList[ExtIndex]);
          DeleteValueWithRegTool(RootKey,
            'Software\Classes\' + Extension + '\OpenWithProgids', ProgId, Report);
          Found := True;
        end;
      end;
    end;
    Result := Found;
  finally
    IdList.Free;
    RootList.Free;
    ExtList.Free;
  end;
end;

function VerifyAbsent(const Report: TStringList): Boolean;
var
  NameLists: TStringList;
  Entries: TStringList;
  Roots: TStringList;
  ListIndex: Integer;
  NameIndex: Integer;
  RootIndex: Integer;
  Candidate: String;
  Remaining: Boolean;
begin
  Result := False;
  Remaining := False;
  NameLists := TStringList.Create;
  Entries := TStringList.Create;
  Roots := BuildProgramRoots;
  try
    NameLists.CommaText := LegacyFolderList + ',' + LegacyDataList;
    Roots.AddStrings(BuildDataRoots);
    for ListIndex := 0 to NameLists.Count - 1 do
    begin
      Entries.Clear;
      Entries.CommaText := Trim(NameLists[ListIndex]);
      for NameIndex := 0 to Entries.Count - 1 do
        for RootIndex := 0 to Roots.Count - 1 do
        begin
          Candidate := Roots[RootIndex] + '\' + Trim(Entries[NameIndex]);
          if DirExists(Candidate) then
          begin
            LogLine(Report, 'STILL PRESENT: ' + Candidate);
            Remaining := True;
          end;
        end;
    end;
    Result := not Remaining;
  finally
    NameLists.Free;
    Entries.Free;
    Roots.Free;
  end;
end;

procedure RemoveLegacyProducts;
var
  Report: TStringList;
  ProgramRoots: TStringList;
  DataRoots: TStringList;
  ShortcutRoots: TStringList;
  Summary: String;
begin
  Report := TStringList.Create;
  ProgramRoots := BuildProgramRoots;
  DataRoots := BuildDataRoots;
  ShortcutRoots := BuildShortcutRoots;
  try
    SweepFolders(LegacyFolderList, ProgramRoots, Report);
    SweepFolders(LegacyFolderList, DataRoots, Report);
    SweepFolders(LegacyDataList, DataRoots, Report);
    SweepShortcuts(LegacyProductList, ShortcutRoots, Report);
    SweepRegistry(LegacyFolderList, Report);
    SweepAssociations(LegacyProgIdList, Report);
    if Report.Count = 0 then
      LogLine(Report, 'No previous Vernayf or GhitaBrowser installation was found.');
    if VerifyAbsent(Report) then
      LogLine(Report, 'Verified: no legacy program folder, data folder or shortcut remains.')
    else
      LogLine(Report, 'Some legacy items could not be removed and are listed above.');
    Summary := Report.Text;
    if (not WizardSilent) and (Trim(Summary) <> '') then
      MsgBox('Cleanup of previous versions' + #13#10 + #13#10 + Summary,
        mbInformation, MB_OK);
  finally
    Report.Free;
    ProgramRoots.Free;
    DataRoots.Free;
    ShortcutRoots.Free;
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssInstall then
    RemoveLegacyProducts;
end;
