# Ascension-Server

「각성과 탐욕의 탑」 전용 게임 서버. 클라이언트 저장소(Ascension-Team)와는 **프로토콜 파일만** 공유한다.

- 언어/도구: C++26(`-std=c++26`), GCC, CMake + Ninja, WSL(Ubuntu) + VSCode
- 네트워크: standalone asio 1.38 (Linux epoll / Windows IOCP), C++20 코루틴
- 현재 단계: **P0** — 접속 → 로그인 → 팀 층 인스턴스 입장(최대 3명) → 이동 동기화

## 폴더 구조

```
protocol/AscensionProtocol.h   패킷 정의 원본 (C++20 범위만 사용, 클라이언트가 복사해 감)
src/net/      Session(연결 하나), Server(accept), Packet(버퍼 도우미)
src/game/     Room(층 인스턴스 하나), RoomManager(teamCode → Room)
src/core/     로그, 서버 시계
tools/bot/    헤드리스 봇 클라이언트 (동작 확인 + 이후 부하 테스트)
```

## 스레드 구조

```
io_context  ←  스레드 N개가 run()
 ├─ Session strand (연결마다 하나)   읽기 → 패킷 해석 → Room에 post / 쓰기 대기열
 └─ Room strand    (층마다 하나)     입장·퇴장·이동 검증 → 20Hz 틱에 이동 묶음 브로드캐스트
```

- 한 strand 안의 작업은 순서대로 하나씩 실행되므로 **Session·Room 내부 상태에는 락이 없다.**
- 서로 다른 Room은 다른 strand라서 여러 스레드에서 동시에 돌아간다. 이후 층 인스턴스 소유권·태스크 그래프 구조를 이 단위 위에 올린다.
- 락은 `RoomManager` 한 곳뿐이다. 정원 확인을 뮤텍스 안에서 끝내서, 두 명이 동시에 마지막 자리에 들어와도 한 명만 성공한다.
- 브로드캐스트 버퍼는 `shared_ptr`로 공유해 받는 사람 수만큼 복사하지 않는다.
- 송신 대기열이 1024개를 넘으면(받지 못하는 클라이언트) 연결을 끊는다.

## 프로토콜 규칙

- 모든 패킷은 `PacketHeader{size, id}`로 시작한다. `size`는 헤더 포함 전체 길이(최대 4096)다.
- 고정 레이아웃(`#pragma pack(1)`, 리틀 엔디언)이고, 크기는 `static_assert`로 고정해 GCC/MSVC 배치 차이를 컴파일 시간에 잡는다.
- 이 파일은 MSVC로 빌드되는 언리얼에서도 쓰므로 **C++20 기능까지만** 쓴다. C++26 기능은 서버 내부 코드에만 쓴다.
- 패킷을 바꾸면 `kVersion`을 올리고, 클라이언트 저장소의 `Source/AscensionPrototype/AscensionProtocol.h`를 같은 내용으로 갱신한다. 버전이 다르면 로그인을 거부한다.

| 패킷 | 방향 | 내용 |
|---|---|---|
| `C_Login` / `S_Login` | C→S / S→C | 버전·이름 → playerId |
| `C_EnterRoom` / `S_EnterRoom` | | teamCode → 결과, 시작 위치, 기존 멤버 목록 |
| `S_PlayerJoined` / `S_PlayerLeft` | S→C | 같은 층 멤버 변화 |
| `C_Move` | C→S | 15Hz 위치·속도·방향·애니메이션 상태 |
| `S_MoveBatch` | S→C | 20Hz, 바뀐 멤버만 묶어서 |
| `S_Correction` | S→C | 이동 거부 → 이 위치로 되돌릴 것 |
| `C_Ping` / `S_Pong` | | 왕복 지연 측정 |

### 이동 검증 (P0)

P0는 클라이언트가 위치를 보내고 서버가 검증한다. 서버에 충돌 지형이 없어서 서버가 직접 이동을 계산할 수 없기 때문이다.
- **토큰 버킷**: 시간이 지나면 이동 가능 거리가 1100cm/s로 쌓이고(최대 0.5초 분량), 이동할 때 그만큼 쓴다. 패킷 여러 개가 한꺼번에 도착해도(회피 중 네트워크 몰림) 앞서 쌓인 거리로 통과하고, 순간이동은 거부된다.
- 맵 범위(±2800 × ±2400cm)를 벗어나거나 NaN이면 거부한다.

다음 단계에서 서버 충돌 지형을 만들면 **입력 전송 + 서버 이동 계산**으로 바꾼다(합의한 "판정은 서버" 원칙).

## 개발 환경 준비 (처음 한 번)

1. **WSL 설치**: Windows PowerShell(관리자)에서 `wsl --install -d Ubuntu`를 실행하고 재부팅한다.
2. **도구 설치**: WSL(Ubuntu) 터미널에서
   ```bash
   sudo apt update
   sudo apt install -y build-essential cmake ninja-build git
   g++ --version
   ```
3. **GCC 16 설치**: `apt-cache policy g++-16`으로 설치 가능한지 확인하고, 있으면 `sudo apt install -y g++-16`로 설치한다. 없으면 배포판 버전에 맞는 방법(배포판 업그레이드나 직접 빌드)이 필요하다. P0 코드는 GCC 14 이상이면 빌드된다(GCC 14.2로 검증).
4. **저장소 위치**: WSL 홈(`~/`)에 둔다. `/mnt/c/...`의 Windows 폴더는 WSL에서 파일 접근이 매우 느리다.
   ```bash
   cd ~ && git clone <서버 저장소 주소> Ascension-Server
   ```
5. **VSCode 연결**: Windows의 VSCode에 확장 **WSL**, **CMake Tools**, **C/C++**를 설치한다(`.vscode/extensions.json`에 추천 목록이 있다). WSL 터미널에서 `cd ~/Ascension-Server && code .`를 실행하면 WSL에 연결된 VSCode가 열린다.

## 빌드와 실행

```bash
export CXX=g++-16                 # 처음 configure 때만 필요. 다른 이름이면 그 이름으로
cmake --preset debug              # build/debug 에 구성 (asio는 자동으로 받아 온다)
cmake --build --preset debug
./build/debug/ascension_server --port 7777 --threads 4
```

VSCode에서는 하단 상태 표시줄의 CMake Tools에서 프리셋 `debug`를 고르고 빌드해도 된다.

### 봇으로 확인

서버를 켠 채로 다른 WSL 터미널에서:

```bash
./build/debug/ascension_bot --bots 7 --seconds 5            # 3+3+1명 팀 배정·이동 수신
./build/debug/ascension_bot --bots 4 --team-size 4          # 4번째는 정원 초과(RoomFull)
./build/debug/ascension_bot --bots 3 --cheat                # 순간이동 → S_Correction
./build/debug/ascension_bot --bots 6 --burst                # 회피 몰림 → 거부되면 안 됨
```

각 봇이 `PASS/FAIL`을 출력하고, 하나라도 실패하면 종료 코드 1을 돌려준다.

데이터 경합 검사는 `cmake --preset tsan && cmake --build --preset tsan`으로 빌드한 서버에 같은 봇을 붙인다.

### 2026-10-28 검증 결과 (Linux 컨테이너, GCC 14.2)

| 항목 | 결과 |
|---|---|
| 경고 없는 빌드(`-Wall -Wextra -Wpedantic`) | PASS |
| 봇 7개: 팀 3/3/1 배정, 같은 팀 이동 전부 수신, 보정 0 | PASS |
| 정원 초과: 4번째 RoomFull | PASS |
| 40m 순간이동 거부(S_Correction 1회) | PASS |
| 회피 크기 이동 3개 동시 도착 허용 | PASS |
| ThreadSanitizer, 스레드 4개, 봇 60개 + 위 시나리오 | 경고 0 |
| 프로토콜 헤더 C++20 단독 컴파일(GCC·Clang `-Werror`) | PASS |

언리얼 클라이언트 빌드와 실제 두 클라이언트 접속은 아직 Windows에서 확인하지 않았다.

## 언리얼 클라이언트에서 접속하기

클라이언트 저장소의 `Docs/온라인_P0_실행.md`를 따른다. 요약:

```powershell
.\Scripts\OpenOnline.ps1 -BuildRoot C:\Builds\AscensionPrototype -Name player1 -Team 1
.\Scripts\OpenOnline.ps1 -BuildRoot C:\Builds\AscensionPrototype -Name player2 -Team 1
```

### 네트워크 (WSL2)

- **같은 PC**: 서버가 WSL에 있어도 Windows의 `127.0.0.1:7777`로 바로 접속된다(WSL 기본 localhost 전달).
- **다른 PC(팀원)에서 접속**: 서버 PC에서 아래를 한 번 설정한다.
  1. `C:\Users\<사용자>\.wslconfig` 파일을 만들고 다음 두 줄을 넣는다(Windows 11 22H2 이상).
     ```ini
     [wsl2]
     networkingMode=mirrored
     ```
  2. PowerShell에서 `wsl --shutdown`을 실행한 뒤 WSL을 다시 연다.
  3. PowerShell(관리자)에서 Windows 방화벽과 WSL용 Hyper-V 방화벽을 연다.
     ```powershell
     New-NetFirewallRule -DisplayName "Ascension Server" -Direction Inbound -Protocol TCP -LocalPort 7777 -Action Allow
     New-NetFirewallHyperVRule -Name AscensionServer -DisplayName "Ascension Server" -Direction Inbound -VMCreatorId '{40E0AC32-46A5-438A-A0B2-2B479E8F2E90}' -Protocol TCP -LocalPorts 7777
     ```
  4. 팀원은 `-Server <서버 PC의 IPv4>:7777`로 접속한다. IP는 서버 PC에서 `ipconfig`로 확인한다.

## 작업 규칙

- `main`에 직접 커밋하지 않는다. 기능마다 브랜치를 만들고(`feature/...`, `fix/...`, `docs/...`) Pull Request로 합친다.
- 커밋 제목은 클라이언트 저장소와 같은 한국어 형식이다: `기능:`, `수정:`, `문서:`, `검증:`.
- 서버 변경에는 봇 시나리오 결과를 함께 기록한다.

## 다음 단계

1. 서버 충돌 지형: 클라이언트 맵 생성 스크립트(`CreateMap.py`)의 데이터를 서버용으로 내보내기
2. 이동을 서버 권한으로 전환(입력 전송, 서버 이동 계산, 클라이언트 예측·보정)
3. 몬스터 스폰·추적을 서버로 이전 → 봇 부하 테스트로 층당 몬스터 수 목표 잡기
