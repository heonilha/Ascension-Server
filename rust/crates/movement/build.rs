// 공유 이동 코드(protocol/AscensionMovement.h)를 C++ 그대로 컴파일해 붙인다.
// 언리얼 클라이언트와 같은 원본을 쓰기 위해서다. Windows에서는 Visual Studio의 C++ 도구가 필요하다.
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let protocol = manifest.join("../../../protocol");
    let shim = manifest.join("native/movement_shim.cpp");

    println!("cargo:rerun-if-changed={}", shim.display());
    println!("cargo:rerun-if-changed={}", protocol.join("AscensionMovement.h").display());
    println!("cargo:rerun-if-changed={}", protocol.join("AscensionProtocol.h").display());

    let mut build = cc::Build::new();
    build.cpp(true).std("c++20").include(&protocol).file(&shim);
    // 부동소수점 결과가 CMake로 빌드한 C++ 서버·봇·클라이언트와 같아야 한다.
    // 빠른 수학 옵션이나 곱셈-덧셈 합치기(FMA)를 켜지 않는다. tests/golden.rs가 결과를 비교한다.
    if build.get_compiler().is_like_msvc() {
        build.flag("/utf-8").flag("/permissive-").flag("/fp:precise");
    } else {
        build.flag("-ffp-contract=off");
    }
    build.compile("asc_movement_shim");
}
