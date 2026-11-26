# GodseonClip 개인정보처리방침

시행일: 2026-11-26

GodseonClip은 영상 자르기 · 구간 제거 · H.265 / AV1 변환을 하는 데스크톱 프로그램입니다.

## 네트워크로 보내는 정보

- GodseonClip은 **네트워크로 어떤 정보도 보내지 않습니다.** 영상은 그 컴퓨터에 설치된 FFmpeg로만 처리되고, 개발자나 다른 서버로 보내지 않습니다.
- 사용 통계, 광고, 분석 · 추적 도구, 오류 보고 전송이 없습니다. 자동 업데이트 확인도 아직 없습니다.
- 설정 화면의 설치 안내 링크(예: brew.sh, FFmpeg 빌드 페이지)는 누를 때만 기본 브라우저로 열립니다.
- Windows 설치 파일은 Microsoft Edge WebView2 런타임이 없을 때만 Microsoft 서버에서 그 설치 프로그램을 받습니다.

## 컴퓨터에 저장하는 정보

- 설정(직접 지정한 FFmpeg 경로, 동시 작업 수, 언어)은 그 컴퓨터의 앱 설정 폴더 `settings.json`에만 저장됩니다.
- 미리보기용 프록시 영상과 예상 크기 시험 파일은 그 컴퓨터의 앱 캐시 폴더에만 만들어집니다 (예상 크기 시험 파일은 재자마자 지움).
- 결과 영상은 사용자가 고른 폴더(기본은 원본과 같은 폴더)에 새 파일로 저장되고, 원본 파일은 바꾸지 않습니다.

## 바뀌는 경우

이 문서가 바뀌면 이 저장소의 `PRIVACY.md`를 고치고 시행일을 바꿉니다.

## 문의

GitHub 저장소의 Issues로 문의하세요.

---

## Privacy Policy (English)

Effective: 2026-11-26

GodseonClip is a desktop program for trimming, cutting out and converting video to H.265 / AV1.

- **Nothing is sent over the network.** Video is processed only by the FFmpeg installed on your computer and is never sent to the developer or any other server.
- No usage statistics, advertising, analytics, tracking or crash reporting. No automatic update checks yet.
- Install-guide links in the settings screen (e.g. brew.sh, FFmpeg build pages) open in your default browser only when you click them.
- The Windows installer downloads the Microsoft Edge WebView2 Runtime installer from Microsoft only if the runtime is missing.
- Settings (a chosen FFmpeg path, parallel jobs, language) are stored only in `settings.json` in the app's config folder on that computer. Preview proxies and size-estimate test files stay in the app's cache folder (test files are deleted right after measuring). Results are saved as new files in the folder you choose; originals are never changed.
- **Contact**: GitHub Issues of this repository.
