# Ascension-Server

「각성과 탐욕의 탑」 전용 게임 서버. 클라이언트 저장소(Ascension-Team)와는 **프로토콜 파일만** 공유한다.

- 언어/도구: C++26(`-std=c++26`), GCC, CMake + Ninja, WSL(Ubuntu) + VSCode
- 네트워크: standalone asio 1.38 (Linux epoll / Windows IOCP), C++20 코루틴
- 현재 단계: **P1 진행 중** — P0(접속 → 로그인 → 팀 층 입장 → 이동 동기화) 위에 서버 회피 판정과 지연 보정을 올렸다. PvE 방(최대 3명)과 PvP 방(최대 16명)을 모드별 규칙으로 나눈다.

## 폴더 구조

```
protocol/AscensionProtocol.h   패킷 정의 원본 (C++20 범위만 사용, 클라이언트가 복사해 감)
src/net/      Session(연결 하나), Server(accept), Packet(버퍼 도우미)
src/game/     Room(층 인스턴스 하나), RoomManager((모드, teamCode) → Room), CollisionMap(충돌 지형), CombatRules(모드별 판정 규칙)
src/core/     로그, 서버 시계
tools/bot/    헤드리스 봇 클라이언트 (동작 확인 + 이후 부하 테스트)
tools/tests/  단위 시험 (충돌 지형)
data/         충돌 지형 (클라이언트에서 내보낸 파일, 손으로 고치지 않음)
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
| `C_EnterRoom` / `S_EnterRoom` | | 모드(PvE/PvP)·teamCode → 결과, 시작 위치, 정원, 모드 규칙, 서버 시각, 기존 멤버 목록 |
| `S_PlayerJoined` / `S_PlayerLeft` | S→C | 같은 층 멤버 변화 |
| `C_Move` | C→S | 15Hz 위치·속도·방향·애니메이션 상태 |
| `S_MoveBatch` | S→C | 20Hz, 바뀐 멤버만 묶어서 |
| `S_Correction` | S→C | 이동 거부 → 이 위치로 되돌릴 것 |
| `C_Ping` / `S_Pong` | | 왕복 지연 측정·서버 시계 추정. `C_Ping`에 직전 왕복 시간을 실어 보낸다 |
| `C_Dodge` | C→S | 회피 시작: 누른 시각(서버 시계 추정값) |
| `C_Attack` | C→S | PvP 근접 공격: 누른 시각·방향 |
| `S_AttackStart` | S→C | 서버가 정한 공격: 시작·명중 시각, 범위 |
| `S_HitResult` | S→C | 범위 안 플레이어별 판정(피격/회피), 피해, 인정한 회피 시각·되감기 |

### 충돌 지형 (P1)

서버는 `data/CombatLab.txt`(장애물 45개: 2D 볼록 다각형 + 높이 범위)를 읽어 이동과 공격을 지형으로도 검사한다(`src/game/CollisionMap.*`).

- **데이터 출처**: 클라이언트 저장소 `Scripts/ExportServerCollision.py`가 실제 맵의 충돌 액터 변환값에서 뽑는다. 손으로 고치지 않고, 맵을 바꾸면 다시 내보내 `data/`에 그대로 복사한다. 클라이언트는 `-AscensionCollisionVerify`로 같은 파일을 게임 물리와 40cm 격자 비교해 검증한다.
- **막히는 기준**: 윗면이 계단 높이(45cm)보다 높고, 캐릭터 캡슐 높이 범위(중심 ±88cm)와 겹치는 장애물. 그래서 기둥 받침(40cm)은 올라설 수 있고, 머리 위 상인방·제단 덮개 아래로는 지나갈 수 있다.
- **이동**: 도착 위치가 장애물에서 24cm 안이거나(캡슐 32cm에서 8cm 여유), 이전 위치에서 반지름 16cm 원을 10cm 간격으로 밀어 장애물에 걸리면 `S_Correction`으로 되돌린다(기둥 뚫기 방지). 기둥 둘레를 정상 속도로 도는 이동은 통과한다.
- **공격 엄폐**: 공격 시작점(근접은 공격자 위치, 내려찍기는 범위 중심)에서 대상까지 선분을 장애물이 가리면 맞지 않는다.
- **실행 위치**: 저장소 루트에서 실행하면 기본 경로로 읽는다. 다른 곳에서는 `--map <경로>`, 끄려면 `--map none`. 파일을 못 읽으면 서버가 시작하지 않는다.
- 다음 단계(서버 이동 계산·몬스터 길찾기)도 이 데이터를 쓴다.

### 회피 판정과 지연 보정 (P1)

무적 시간(PvE 0.15초)이 왕복 지연보다 짧아서, 서버가 도착 순서대로만 판정하면 "피했는데 맞는" 일이 생긴다. 그래서 다음처럼 판정한다(`src/game/CombatRules.h`).

1. **명중 시각은 서버가 정한다.** 공격 시작 때 `S_AttackStart`로 명중 시각을 알리고, 클라이언트는 서버 시계 기준으로 예고를 재생한다. 플레이어가 보는 명중 순간이 서버 명중 시각과 같아져서, 남는 지연은 회피 입력이 서버로 가는 시간뿐이다.
2. **회피 입력에 누른 시각을 붙인다.** 서버는 그 시각을 `[도착 − 허용 되감기, 도착]`으로 자른다. 허용 되감기 = `min(왕복/2 + 30ms, 모드 상한)`. 미래 시각과 상한보다 먼 과거는 인정하지 않는다.
3. **판정 유예.** 명중 시각이 지나도 바로 확정하지 않고 `명중 시각 + 허용 되감기`까지 기다린다. 그 안에 도착한 회피는 모두 반영되고, HP를 깎았다 되돌리는 롤백이 없다.
4. **위치는 이력에서 찾는다.** 플레이어 위치를 도착 시각과 함께 1초 보관하고, `명중 시각 + 한쪽 지연` 표본으로 범위 안인지 본다.
5. **재사용 대기는 인정한 시각끼리 비교한다.** 시각을 앞당겨도 직전 회피를 앞지를 수 없다.

| 모드 | 무적 | 되감기 상한 | 공격 준비 | 이유 |
|---|---|---|---|---|
| PvE | 150ms | 200ms | (몬스터 패턴별, 시험 내려찍기 800ms) | 1인 데모 손맛 유지, 협동이라 지연 큰 팀원에게 관대 |
| PvP | 300ms | 100ms | 350ms | 시각 조작 이득을 100ms로 제한. 무적을 늘려 왕복 100ms가 무적의 1/3을 넘지 않게 |

- 시각을 조작하거나 왕복 시간을 부풀려도 얻는 이득은 **상한까지**다. 상한만큼 지연이 큰 정직한 플레이어와 같은 조건이다.
- PvP 공격은 서버 도착 시각 + 공격 준비 시간에 명중한다. 공격자 쪽은 되감지 않는다(준비 시간이 지연을 흡수). 대신 방어자에게 보이는 준비 시간은 자기 쪽 지연만큼 짧아진다.
- 서버 HP는 판정 기록용이다. P1에는 사망·부활 흐름이 없어서 HP가 0이 되면 서버 쪽은 100으로 채운다(클라이언트는 기존 쓰러짐 처리).

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
./build/debug/ascension_bot --bots 1 --wall --seconds 6      # 기둥 뚫고 걷기 → S_Correction(충돌 지형)
./build/debug/ascension_terrain_test                        # 충돌 지형 단위 시험(서버 없이)
```

각 봇이 `PASS/FAIL`을 출력하고, 하나라도 실패하면 종료 코드 1을 돌려준다.

회피 판정 시험은 서버를 `--pve-slam-ms 1500`(PvE 방에 1.5초마다 서버 내려찍기 시험 패턴)으로 켠 뒤:

```bash
./build/debug/ascension_bot --dodge-test --bots 1 --seconds 14 --latency 100           # PvE, 한쪽 지연 100ms
./build/debug/ascension_bot --dodge-test --dodge-cheat --rtt-lie 2000 --bots 1 --seconds 16  # 시각 조작 + 왕복 부풀리기
./build/debug/ascension_bot --mode pvp --dodge-test --bots 2 --team-size 2 --seconds 14 --latency 50  # PvP: 0번 봇이 공격
```

- 방어 봇은 공격마다 "무적 앞부분 / 명중 직전 / 명중 뒤 / 너무 이름 / 누르지 않음" 순서로 회피를 누르고, 서버 결과를 같은 규칙으로 계산한 기대값과 비교한다. 경계 25ms 안은 시계 추정 오차 때문에 비교에서 뺀다.
- `--latency`는 송신·수신 각각에 인공 지연을 넣는다. `--dodge-cheat`는 늦게 누르고 시각을 앞당겨 보내는 조작을 섞는다.
- 시험 전체: 클라이언트 저장소 기록(Verification/RESULTS.md)의 2026-11-03 항목 참고.

데이터 경합 검사는 `cmake --preset tsan && cmake --build --preset tsan`으로 빌드한 서버에 같은 봇을 붙인다. 최신 WSL 커널에서 TSan이 `unexpected memory mapping`으로 바로 끝나면 `setarch $(uname -m) -R ./build/tsan/ascension_server ...`처럼 주소 무작위화를 끄고 실행한다.

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

이후 Windows에서 언리얼 클라이언트 1개 + 봇 접속을 확인했다(클라이언트 저장소 Verification/RESULTS.md 2026-10-29). 렌더링 화면의 두 클라이언트 동시 접속은 아직 확인하지 않았다.

### 2026-11-03 P1 검증 결과 (WSL Ubuntu 24.04, GCC 14.2)

| 항목 | 결과 |
|---|---|
| 경고 없는 빌드(debug·tsan) | PASS |
| 기존 시나리오 4개(팀 배정·정원 초과·순간이동·회피 몰림) | PASS |
| PvE 회피 판정, 한쪽 지연 0/40/100/150/300ms | 비교 60건 불일치 0 (PvP 포함 경계 제외 5건) |
| PvP 회피 판정, 한쪽 지연 0/50/100/150ms | 비교 51건 불일치 0 |
| 시각 조작·왕복 부풀리기: 상한 안만 인정, 상한 밖 지각은 피격 | PASS |
| ThreadSanitizer, 스레드 4개, 봇 60개 + 회피 시험 동시 | 경고 0 |
| 언리얼 클라이언트(Windows) PvE·PvP 자동 회피: 회피/피격 의도와 일치 | PASS |

이 PC의 WSL 시계가 Windows보다 간헐적으로 수 % 빠르게 흘러, Windows 클라이언트의 서버 시계 추정에 10~40ms 오차가 남았다(허용 여유 안). 운영 서버 환경에서 다시 잰다.

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

P1 남은 항목(회피 판정·지연 보정, 충돌 지형은 위에서 완료):

1. 이동을 서버 권한으로 전환(입력 전송, 서버 이동 계산, 클라이언트 예측·보정)
2. 몬스터 스폰·추적·넉백·경직을 서버로 이전, 층당 최대 48마리 증원·보충 → 봇 부하 테스트
3. 수문장 패턴을 서버 상태 머신으로(내려찍기 시험 패턴을 실제 패턴으로 교체)
4. 검기 투사체 서버 스윕(관통 순서, 같은 적 중복 피격 방지)
5. 경험치 구슬 소유권, 각성 후보 RNG, 선택 검증
