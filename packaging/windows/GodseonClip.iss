; GodseonClip 윈도우 설치 파일 (Inno Setup 6)
; 먼저 `npm run tauri build -- --no-bundle` 로 만든 src-tauri\target\release\godseon-clip.exe 를
; build\windows\GodseonClip.exe 로, LICENSE 를 build\windows\LICENSE.txt 로 복사한 뒤 (GitHub Actions 가 한다):
;   iscc /DAppVersion=1.2.3 packaging\windows\GodseonClip.iss          (미리 릴리즈는 /DAppVersion=1.2.3-beta /DAppNumVersion=1.2.3)
; 관리자 권한 없이 사용자 폴더(%LOCALAPPDATA%\Programs\GodseonClip)에 설치한다. FFmpeg 는 넣지 않는다 (사용자 설치본을 앱이 찾는다).
; WebView2 런타임이 없으면 설치 마지막에 Microsoft 에서 부트스트래퍼를 받아 사용자 범위로 설치한다 (윈도우 11 · 최신 10 에는 이미 있다).
;
; /DSignUninstaller: 제거 프로그램을 밖에서 서명한다 (GitHub Actions 의 SignPath 서명 — GodseonPDF · GodseonSync 와 같은 방식).
; 서명 도구 없이 SignedUninstaller=yes 면 처음 컴파일은 build\uninst\uninst-<Inno 버전>-<해시>.e32 를 만들고 '서명 필요' 로 멈춘다
; → 그 파일을 서명해 두고 다시 컴파일하면 Inno Setup 이 서명을 확인해(서명만 빼면 원래와 같은지) 끼워 넣는다.
; 버전 정보가 바뀌면 해시도 바뀌어 릴리즈마다 새로 서명한다.
; 이 파일은 BOM 이 있는 UTF-8 이다 (한국어 안내 문구 때문 — BOM 을 지우면 한글이 깨진다).

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
; 윈도우 버전 정보는 숫자만 받는다: 태그가 1.1.0-beta 면 /DAppNumVersion=1.1.0 (Actions 가 넘긴다 — SignPath 에 넘기는 버전과 같게)
#ifndef AppNumVersion
  #define AppNumVersion AppVersion
#endif

[Setup]
AppId={{EEDFCF4F-6B35-4CE9-9A4B-714E33397E65}
AppName=GodseonClip
AppVersion={#AppVersion}
AppVerName=GodseonClip {#AppVersion}
AppPublisher=afy0817
AppPublisherURL=https://github.com/afy0817/GodseonClip
AppSupportURL=https://github.com/afy0817/GodseonClip/issues
DefaultDirName={localappdata}\Programs\GodseonClip
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\release
OutputBaseFilename=GodseonClip-{#AppVersion}-windows-setup
SetupIconFile=..\..\src-tauri\icons\icon.ico
UninstallDisplayIcon={app}\GodseonClip.exe
UninstallDisplayName=GodseonClip
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; 설치 파일 · 제거 프로그램의 버전 정보 (코드 서명이 제품 이름 · 버전을 확인한다)
VersionInfoVersion={#AppNumVersion}
VersionInfoTextVersion={#AppNumVersion}
VersionInfoProductName=GodseonClip
VersionInfoProductVersion={#AppNumVersion}
VersionInfoProductTextVersion={#AppNumVersion}
VersionInfoCompany=afy0817
VersionInfoDescription=GodseonClip Setup
VersionInfoCopyright=Copyright (c) 2026 afy0817. MIT License.
#ifdef SignUninstaller
SignedUninstaller=yes
SignedUninstallerDir=..\..\build\uninst
#endif

[Languages]
#ifexist AddBackslash(CompilerPath) + "Languages\Korean.isl"
Name: "korean"; MessagesFile: "compiler:Languages\Korean.isl"
#endif
Name: "english"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
english.WebView2Failed=GodseonClip needs the Microsoft Edge WebView2 Runtime, but it could not be installed automatically. Install it from https://developer.microsoft.com/microsoft-edge/webview2/ and start GodseonClip again.
#ifexist AddBackslash(CompilerPath) + "Languages\Korean.isl"
korean.WebView2Failed=GodseonClip을 실행하려면 Microsoft Edge WebView2 런타임이 필요한데 자동으로 설치하지 못했습니다. https://developer.microsoft.com/microsoft-edge/webview2/ 에서 설치한 뒤 GodseonClip을 다시 실행하세요.
#endif

[Files]
Source: "..\..\build\windows\GodseonClip.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\build\windows\LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\GodseonClip"; Filename: "{app}\GodseonClip.exe"

[Run]
Filename: "{app}\GodseonClip.exe"; Description: "{cm:LaunchProgram,GodseonClip}"; Flags: nowait postinstall skipifsilent

[Code]
// WebView2 런타임 (Tauri 화면이 쓰는 Edge 엔진). 컴퓨터 전체 설치는 HKLM 의 WOW6432Node, 사용자 설치는 HKCU 에 pv(버전)가 있다
const
  WebView2Client = 'Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';
  WebView2Bootstrapper = 'https://go.microsoft.com/fwlink/p/?LinkId=2124703';

function WebView2VersionAt(RootKey: Integer; SubKey: String): String;
var
  Version: String;
begin
  Version := '';
  if not RegQueryStringValue(RootKey, SubKey, 'pv', Version) then
    Version := '';
  Result := Version;
end;

function WebView2Installed: Boolean;
var
  Version: String;
begin
  Version := WebView2VersionAt(HKLM, 'SOFTWARE\WOW6432Node\' + WebView2Client);
  if (Version = '') or (Version = '0.0.0.0') then
    Version := WebView2VersionAt(HKCU, 'Software\' + WebView2Client);
  Result := (Version <> '') and (Version <> '0.0.0.0');
end;

procedure InstallWebView2;
var
  ResultCode: Integer;
begin
  try
    DownloadTemporaryFile(WebView2Bootstrapper, 'MicrosoftEdgeWebview2Setup.exe', '', nil);
    if not Exec(ExpandConstant('{tmp}\MicrosoftEdgeWebview2Setup.exe'), '/silent /install', '', SW_HIDE, ewWaitUntilTerminated, ResultCode)
      or (ResultCode <> 0) then
      SuppressibleMsgBox(CustomMessage('WebView2Failed'), mbInformation, MB_OK, IDOK);
  except
    SuppressibleMsgBox(CustomMessage('WebView2Failed'), mbInformation, MB_OK, IDOK);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and not WebView2Installed then
    InstallWebView2;
end;
