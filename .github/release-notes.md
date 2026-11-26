맥 · 윈도우 · 우분투용 영상 자르기 · 구간 제거 · H.265 / AV1 변환기 GodseonClip __VERSION__.
사용법은 [README](https://github.com/__REPO__#readme) ([English](https://github.com/__REPO__/blob/main/README.en.md)).

### 설치 파일 · Downloads

| OS | 파일 |
|---|---|
| macOS (Apple Silicon · Intel) | `GodseonClip-__VERSION__-macos-universal.dmg` |
| Windows 10 / 11 (x64) | `GodseonClip-__VERSION__-windows-setup.exe` (설치) · `-windows-portable.zip` (압축만 풀어 실행) |
| Ubuntu 22.04 / 24.04 (x64) | `GodseonClip-__VERSION__-linux-amd64.deb` · `.AppImage` |

### 먼저 FFmpeg 6.0 이상을 설치하세요 · Install FFmpeg 6.0+ first

GodseonClip은 FFmpeg를 포함하지 않습니다 — FFmpeg is not bundled.

| OS | |
|---|---|
| macOS | `brew install ffmpeg` |
| Windows | `winget install --id Gyan.FFmpeg -e` |
| Ubuntu 24.04 | `sudo apt install ffmpeg` |
| Ubuntu 22.04 | [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds/releases) linux64-gpl → 앱 설정의 ‘직접 경로 지정’ |

### 서명 · Code signing

- Windows: SignPath Foundation 인증서로 서명합니다 — `GodseonClip.exe` 속성 › **디지털 서명**에 'SignPath Foundation'이 있으면 서명된 파일입니다. 없으면 “Windows의 PC 보호” 창에서 **추가 정보** → **실행** (SmartScreen: More info → Run anyway). [Code signing policy](https://github.com/__REPO__#code-signing-policy)
- macOS: 아직 공증 전입니다 (ad-hoc 서명) — 처음 열 때 막히면 시스템 설정 → 개인정보 보호 및 보안 → **그래도 열기**, 또는 `xattr -dr com.apple.quarantine /Applications/GodseonClip.app`
- Windows · Ubuntu 빌드는 아직 실기기 검증 전입니다 — Windows and Ubuntu builds are not verified on real machines yet.

라이선스 · License: [MIT](https://github.com/__REPO__/blob/main/LICENSE)
