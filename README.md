# Ascension-Server

「각성과 탐욕의 탑」 전용 게임 서버. 클라이언트 저장소(Ascension-Team)와는 **`protocol/` 폴더의 파일만** 공유한다.

**Windows에서 바로 빌드하고 실행한다.** Visual Studio만 있으면 되고 WSL은 필요 없다 → [빌드와 실행](#빌드와-실행)

- 언어/도구: C++23, MSVC(Visual Studio 2022 이상) + CMake. Linux + GCC 14 이상에서도 빌드된다(ThreadSanitizer 검사용, 선택)
- 네트워크: standalone asio 1.38 + C++20 코루틴. Windows에서는 IOCP 위에서 돈다(Linux는 epoll)
- 현재 단계: **P1 진행 중** — 서버 권한 이동(입력 전송 + 공유 이동 코드 + 클라이언트 예측·재조정), 충돌 지형, 서버 회피 판정과 지연 보정. PvE 방(최대 3명)과 PvP 방(최대 16명)을 모드별 규칙으로 나눈다.

## 폴더 구조

```
protocol/AscensionProtocol.h   패킷 정의 원본 (C++20 범위만 사용, 클라이언트가 복사해 감)
protocol/AscensionMovement.h   이동 계산 원본 (서버·봇·클라이언트가 같은 코드로 이동을 계산, 클라이언트가 복사해 감)
src/net/      Session(연결 하나), Server(accept), Packet(버퍼 도우미)
src/game/     Room(층 인스턴스 하나), RoomManager((모드, teamCode) → Room), CollisionMap(충돌 지형), CombatRules(모드별 판정 규칙)
src/core/     로그, 서버 시계, 운영체제별 준비(Platform.h)
tools/bot/    헤드리스 봇 클라이언트 (동작 확인 + 이후 부하 테스트)
tools/tests/  단위 시험 (충돌 지형)
data/         충돌 지형 (클라이언트에서 내보낸 파일, 손으로 고치지 않음)
Scripts/      Windows용 빌드·실행·시험 스크립트
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
- `protocol/`의 파일은 언리얼에서도 쓰므로 **C++20 기능까지만** 쓴다. C++23 기능은 서버 내부 코드에만 쓴다.
- 패킷을 바꾸면 `kVersion`을 올리고, 클라이언트 저장소의 `Source/AscensionPrototype/AscensionProtocol.h`를 같은 내용으로 갱신한다. 버전이 다르면 로그인을 거부한다.

| 패킷 | 방향 | 내용 |
|---|---|---|
| `C_Login` / `S_Login` | C→S / S→C | 버전·이름 → playerId |
| `C_EnterRoom` / `S_EnterRoom` | | 모드(PvE/PvP)·teamCode → 결과, 시작 위치, 정원, 모드 규칙, 서버 시각, 기존 멤버 목록 |
| `S_PlayerJoined` / `S_PlayerLeft` | S→C | 같은 층 멤버 변화 |
| `C_Input` | C→S | 60Hz 틱 입력(이동 방향·회피)을 30Hz로 묶어서. 바라보는 방향·애니메이션 상태(연출용) |
| `S_MoveAck` | S→C | 마지막으로 처리한 입력 번호와 서버 이동 상태(본인에게만, 20Hz) |
| `S_MoveBatch` | S→C | 20Hz, 바뀐 멤버만 묶어서 |
| `C_Ping` / `S_Pong` | | 왕복 지연 측정·서버 시계 추정. `C_Ping`에 직전 왕복 시간을 실어 보낸다 |
| `C_Dodge` | C→S | 회피 시작: 누른 시각(서버 시계 추정값) |
| `C_Attack` | C→S | PvP 근접 공격: 누른 시각·방향 |
| `S_AttackStart` | S→C | 서버가 정한 공격: 시작·명중 시각, 범위 |
| `S_HitResult` | S→C | 범위 안 플레이어별 판정(피격/회피), 피해, 인정한 회피 시각·되감기 |

### 충돌 지형 (P1)

서버는 `data/CombatLab.txt`(장애물 45개: 2D 볼록 다각형 + 높이 범위)를 읽어 이동과 공격을 지형으로도 검사한다(`src/game/CollisionMap.*`).

- **데이터 출처**: 클라이언트 저장소 `Scripts/ExportServerCollision.py`가 실제 맵의 충돌 액터 변환값에서 뽑는다. 손으로 고치지 않고, 맵을 바꾸면 다시 내보내 `data/`에 그대로 복사한다. 클라이언트는 `-AscensionCollisionVerify`로 같은 파일을 게임 물리와 40cm 격자 비교해 검증한다.
- **막히는 기준**: 윗면이 계단 높이(45cm)보다 높고, 캐릭터 캡슐 높이 범위(중심 ±88cm)와 겹치는 장애물. 그래서 기둥 받침(40cm)은 올라설 수 있고, 머리 위 상인방·제단 덮개 아래로는 지나갈 수 있다.
- **이동**: 서버 이동 계산(아래 "서버 권한 이동")이 반지름 32cm 원을 장애물 밖으로 밀어내고 벽을 따라 미끄러지게 한다. 2D 이동이라 계단 오르기가 없어서, 이동에서는 낮은 받침(40cm)도 막는다.
- **공격 엄폐**: 공격 시작점(근접은 공격자 위치, 내려찍기는 범위 중심)에서 대상까지 선분을 장애물이 가리면 맞지 않는다.
- **파일 위치**: 현재 폴더의 `data/`에서 찾고, 없으면 실행 파일 위쪽 폴더로 올라가며 찾는다(스크립트로 실행하든 Visual Studio에서 F5로 실행하든 된다). 다른 파일은 `--map <경로>`, 끄려면 `--map none`. 파일을 못 읽으면 서버가 시작하지 않는다.
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

### 서버 권한 이동 (P1, 프로토콜 v3)

P0의 "클라이언트가 위치를 보내고 서버가 검증"을 **입력 전송 + 서버 이동 계산**으로 바꿨다.

- **공유 이동 코드**: `protocol/AscensionMovement.h`. 60Hz 고정 틱, 달리기 450cm/s(가속·제동 2048cm/s²), 회피 0.65초 300cm(1인 데모와 같은 거리 곡선), 재사용 58틱, 지형 충돌. 서버·봇·언리얼 클라이언트가 같은 코드를 컴파일해 쓰므로 같은 입력이면 같은 위치가 나온다(GCC 서버 + MSVC 클라이언트 조합에서도 오차 0을 확인했다).
- **클라이언트**: 틱마다 입력을 만들어 자기 화면에서 바로 움직이고(예측), 입력을 번호와 함께 30Hz로 묶어 보낸다. 틱은 게임 시간이 아니라 **실제 시간**으로 센다(게임 시간은 히트 스톱이나 2000fps 초과에서 실제와 달라져 서버와 입력 수가 어긋난다).
- **서버**: 입력을 받은 순서대로 한 틱씩 계산해 위치를 정한다. 같은 방 사람들에게는 `S_MoveBatch`로, 보낸 본인에게는 마지막 처리 번호와 상태를 `S_MoveAck`로 보낸다.
- **재조정**: 클라이언트는 `S_MoveAck` 상태에서 아직 확정되지 않은 입력을 다시 돌린다. 차이는 화면에서 약 0.1초 동안 녹여 순간이동처럼 보이지 않게 한다. 같은 코드라 정상이면 차이가 0이다.
- **속도 조작 방지**: 입력 처리권이 실제 시간 1초에 60 × 1.03개(최대 0.5초 분량)만 쌓인다. 더 빨리 보낸 입력은 버리고(번호는 처리한 것으로 친다) 클라이언트는 서버 상태로 재조정된다. 3% 여유는 긴 끊김 뒤 처리권이 다시 차오르게 하려는 것이고, 그만큼 속도 조작 상한은 달리기의 103%다. 순간이동·벽 통과는 서버가 직접 계산하므로 원천적으로 불가능하다.
- **한계**: 공격 중 이동 잠금은 클라이언트가 이동 입력을 0으로 보내는 방식이다(서버는 공격 상태를 모른다. 이동을 줄이는 쪽이라 이득이 없다). 높이는 바닥에 선 캡슐 중심(90cm)으로 고정한다.


## 빌드와 실행

### 준비 (처음 한 번)

- **Visual Studio 2022 이상**, 워크로드 "C++를 사용한 데스크톱 개발"(CMake 포함). 언리얼 빌드에 쓰는 것과 같은 설치면 된다.
- **Git** (CMake가 asio를 받아 올 때 쓴다).

### 스크립트로 (권장)

PowerShell에서 저장소 폴더로 이동한 뒤:

```powershell
.\Scripts\Build.ps1        # 빌드 → build\windowsin (처음에는 asio를 받아 온다)
.\Scripts\RunServer.ps1    # 빌드 후 서버 실행(포트 7777). Ctrl+C로 종료
.\Scripts\Test.ps1         # 단위 시험 + 봇 시나리오 전체. 줄마다 PASS/FAIL, 약 30초
```

스크립트 실행이 막혀 있으면 `powershell -ExecutionPolicy Bypass -File .\Scripts\RunServer.ps1`처럼 실행한다.

| 스크립트 | 옵션 |
|---|---|
| `Build.ps1` | `-Debug`(디버그 빌드), `-Clean`(build\windows 삭제 후 빌드) |
| `RunServer.ps1` | `-Port 7777`, `-Threads N`, `-PveSlamMs 1500`(PvE 방에 1.5초마다 내려찍기 시험 패턴), `-NoBuild` |
| `Test.ps1` | `-Port 17777`(시험 서버 포트), `-NoBuild`. 로그는 `build\windows	est-logs` |

### Visual Studio에서

"폴더 열기"로 저장소 폴더를 열면 `CMakePresets.json`의 **Windows (MSVC)** 구성이 잡힌다. 시작 항목을 `ascension_server.exe`로 고르고 F5를 누르면 디버깅된다.

### 다른 PC(팀원)에서 접속

서버 PC의 관리자 PowerShell에서 방화벽을 한 번 연다.

```powershell
New-NetFirewallRule -DisplayName "Ascension Server" -Direction Inbound -Protocol TCP -LocalPort 7777 -Action Allow
```

팀원은 서버 PC의 IPv4 주소(`ipconfig`로 확인)로 접속한다. 같은 PC에서는 `127.0.0.1:7777`이다.

### 언리얼 클라이언트에서 접속

클라이언트 저장소의 `Docs/온라인_P0_실행.md`를 따른다. 요약:

```powershell
.\Scripts\OpenOnline.ps1 -BuildRoot C:\Builds\AscensionPrototype -Name player1 -Team 1
.\Scripts\OpenOnline.ps1 -BuildRoot C:\Builds\AscensionPrototype -Name player2 -Team 1
```

### 봇으로 직접 확인

`Test.ps1`이 아래를 모두 돌린다. 하나씩 보고 싶으면 서버를 켠 채로 다른 PowerShell 창에서:

```powershell
cd build\windowsin
.scension_bot --bots 7 --seconds 5                 # 3+3+1명 팀 배정, 이동 예측 오차 0
.scension_bot --bots 4 --team-size 4               # 4번째는 정원 초과(RoomFull)
.scension_bot --bots 3 --cheat                     # 0번 봇이 입력 3배 → 서버 속도는 처리권 상한에 머문다
.scension_bot --bots 6 --burst                     # 매 틱 회피 → 재사용 대기 유지, 예측 오차 0
.scension_bot --bots 1 --wall --seconds 6          # 기둥으로 계속 걷기 → 장애물에 파고들지 않음
.scension_bot --bots 3 --seconds 6 --latency 300   # 지연 300ms에서도 예측 오차 0
.scension_terrain_test                             # 충돌 지형 단위 시험(서버 없이)
```

각 봇이 `PASS/FAIL`을 출력하고, 하나라도 실패하면 종료 코드 1을 돌려준다.

회피 판정 시험은 서버를 `.\Scripts\RunServer.ps1 -PveSlamMs 1500`으로 켠 뒤:

```powershell
.scension_bot --dodge-test --bots 1 --seconds 14 --latency 100                                 # PvE, 한쪽 지연 100ms
.scension_bot --dodge-test --dodge-cheat --rtt-lie 2000 --bots 1 --seconds 16                   # 시각 조작 + 왕복 부풀리기
.scension_bot --mode pvp --dodge-test --bots 2 --team-size 2 --seconds 14 --latency 50          # PvP: 0번 봇이 공격
```

- 방어 봇은 공격마다 "무적 앞부분 / 명중 직전 / 명중 뒤 / 너무 이름 / 누르지 않음" 순서로 회피를 누르고, 서버 결과를 같은 규칙으로 계산한 기대값과 비교한다. 경계 25ms 안은 시계 추정 오차 때문에 비교에서 뺀다.
- `--latency`는 송신·수신 각각에 인공 지연을 넣는다. `--dodge-cheat`는 늦게 누르고 시각을 앞당겨 보내는 조작을 섞는다.
- 언리얼 클라이언트와 같은 방에 봇을 넣을 때는 `--allow-others`를 붙인다(봇이 아닌 플레이어의 이동을 받아도 실패로 치지 않는다).

### Linux에서 빌드 (선택: ThreadSanitizer)

MSVC에는 ThreadSanitizer가 없어서, 데이터 경합 검사는 Linux(GCC 14 이상, 예: WSL)에서 한다. 평소 개발·시험에는 필요 없다.

```bash
cmake --preset linux-tsan && cmake --build --preset linux-tsan     # build/linux-tsan/bin
setarch $(uname -m) -R ./build/linux-tsan/bin/ascension_server --port 7778   # 최신 커널: 주소 무작위화를 꺼야 TSan이 시작된다
```

## 검증 기록

| 날짜 | 환경 | 결과 |
|---|---|---|
| 2026-10-28 | Linux 컨테이너, GCC 14.2 | P0: 팀 배정·정원 초과·순간이동 거부·회피 몰림 PASS, ThreadSanitizer 봇 60개 경고 0 |
| 2026-11-03 | WSL, GCC 14.2 | P1 회피 판정: PvE·PvP 지연 0~300ms 비교 111건 불일치 0, 시각 조작은 상한 안만 인정, TSan 경고 0. 충돌 지형 단위 시험 21개 PASS |
| 2026-11-05 | WSL, GCC 14.2 | P1 서버 권한 이동: 봇·언리얼 클라이언트 예측 오차 0.00cm, 속도 조작·기둥 돌파·회피 연타 PASS, TSan 경고 0 |
| 2026-11-11 | **Windows, MSVC 19.51(VS 2026)** | Windows 전환: `/W4` 경고 0, `Test.ps1` 21개 PASS(회피 판정 비교 110건 불일치 0, 예측 오차 0.00cm). 언리얼 클라이언트 자동 회피가 계획한 75ms 전(74~76ms)에 판정, 시계 오차 1~2ms. Linux GCC 14.2 빌드도 경고 0 |

세부 내용은 클라이언트 저장소 `Verification/RESULTS.md`에 날짜별로 있다. WSL에서 서버를 돌릴 때는 WSL 시계가 Windows보다 수 % 빠르게 흘러 클라이언트의 서버 시계 추정에 10~40ms 오차가 있었다. Windows에서 직접 실행하면서 사라졌다.

## 작업 규칙

- `main`에 직접 커밋하지 않는다. 기능마다 브랜치를 만들고(`feature/...`, `fix/...`, `docs/...`) Pull Request로 합친다.
- 커밋 제목은 클라이언트 저장소와 같은 한국어 형식이다: `기능:`, `수정:`, `문서:`, `검증:`.
- 서버 변경에는 봇 시나리오 결과를 함께 기록한다.

## 다음 단계

P1 남은 항목(회피 판정·지연 보정, 충돌 지형, 서버 권한 이동은 위에서 완료):

1. 몬스터 스폰·추적·넉백·경직을 서버로 이전, 층당 최대 48마리 증원·보충 → 봇 부하 테스트
2. 수문장 패턴을 서버 상태 머신으로(내려찍기 시험 패턴을 실제 패턴으로 교체)
3. 검기 투사체 서버 스윕(관통 순서, 같은 적 중복 피격 방지)
4. 경험치 구슬 소유권, 각성 후보 RNG, 선택 검증
