// 운영체제별 준비와 데이터 파일 찾기. 서버·봇·시험 프로그램이 main 처음에 부른다.
#pragma once

#include <filesystem>
#include <string>

#ifdef _WIN32
#include <windows.h>

#include <timeapi.h>
#endif

namespace asc
{
    // Windows: 타이머 해상도를 1ms로 올리고(기본 약 15.6ms라 20Hz 틱·봇 입력 시각이 흔들린다)
    // 콘솔을 UTF-8로 바꾼다(한글 로그). Linux: 할 일 없음.
    inline void InitPlatform()
    {
#ifdef _WIN32
        timeBeginPeriod(1);
        SetConsoleOutputCP(CP_UTF8);
#endif
    }

    // 데이터 파일을 찾는다: 현재 폴더 기준 → 실행 파일 폴더에서 위로 올라가며.
    // 저장소 루트에서 실행하든 build/<프리셋>/bin에서(Visual Studio F5) 실행하든 같은 파일을 쓴다.
    // 못 찾으면 받은 경로를 그대로 돌려준다(여는 쪽에서 오류를 낸다).
    inline std::string FindDataFile(const char* argv0, const std::string& relativePath)
    {
        namespace fs = std::filesystem;
        std::error_code ec;
        if (fs::exists(relativePath, ec)) return relativePath;
        fs::path directory = fs::absolute(fs::path(argv0 ? argv0 : ""), ec).parent_path();
        for (int depth = 0; depth < 5 && !directory.empty(); ++depth)
        {
            const fs::path candidate = directory / relativePath;
            if (fs::exists(candidate, ec)) return candidate.string();
            if (directory == directory.parent_path()) break;
            directory = directory.parent_path();
        }
        return relativePath;
    }
}
