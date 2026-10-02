//! Pins independently checked against the immutable denoland/deno v2.9.7 release:
//! https://github.com/denoland/deno/releases/tag/v2.9.7
//! GitHub release API archive digests and sizes were checked, then each official
//! archive was downloaded and its sole executable independently hashed.
//! Windows metadata is pinned, not a claim of Windows runtime validation.

pub(super) const VERSION: &str = "2.9.7";

pub(super) struct Asset {
    pub target: &'static str,
    pub archive: &'static str,
    pub archive_sha256: &'static str,
    pub archive_bytes: u64,
    pub executable: &'static str,
    pub binary_sha256: &'static str,
    pub binary_bytes: u64,
}

pub(super) fn current() -> Result<&'static Asset, String> {
    for asset in &ASSETS {
        if asset.target == target() {
            return Ok(asset);
        }
    }
    Err(format!(
        "Automatic notebook Deno installation is unsupported on {}-{}. Set features.code_mode.deno_program to an installed Deno executable",
        std::env::consts::OS,
        std::env::consts::ARCH
    ))
}

fn target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") if cfg!(target_env = "gnu") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") if cfg!(target_env = "gnu") => "aarch64-unknown-linux-gnu",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("windows", "x86_64") if cfg!(target_env = "msvc") => "x86_64-pc-windows-msvc",
        ("windows", "aarch64") if cfg!(target_env = "msvc") => "aarch64-pc-windows-msvc",
        _ => "unsupported",
    }
}

const ASSETS: [Asset; 6] = [
    Asset {
        target: "x86_64-unknown-linux-gnu",
        archive: "deno-x86_64-unknown-linux-gnu.zip",
        archive_sha256: "c6527f24f4b16031d3ae4fa9f658d5f11534c8d84ce7dc8502420280919c3490",
        archive_bytes: 41_596_794,
        executable: "deno",
        binary_sha256: "ce6a052beb97c2b92de67077e3f3924ba7c9661ede0d8dc2f66a116a3c841f21",
        binary_bytes: 95_830_104,
    },
    Asset {
        target: "aarch64-unknown-linux-gnu",
        archive: "deno-aarch64-unknown-linux-gnu.zip",
        archive_sha256: "c832298b1ad4422481334855f6003e0f54145762c5a134f20a489511d2f65bbf",
        archive_bytes: 39_821_318,
        executable: "deno",
        binary_sha256: "5ecc9a6b862d61d4cf8cae3ac5f29488ffc839d675cfd60c11d908121dfe5bf1",
        binary_bytes: 84_971_392,
    },
    Asset {
        target: "x86_64-apple-darwin",
        archive: "deno-x86_64-apple-darwin.zip",
        archive_sha256: "95daaff11c116a52ad54785e7914c8e9c9cdcaba793c5ed929c74ca2d8e6259a",
        archive_bytes: 42_295_422,
        executable: "deno",
        binary_sha256: "325cf9c4b7156f4efd96ad22177a276a4bf4384147d4c9d9f40f9caee28e5719",
        binary_bytes: 97_829_808,
    },
    Asset {
        target: "aarch64-apple-darwin",
        archive: "deno-aarch64-apple-darwin.zip",
        archive_sha256: "5cd46d6268f6f78f5d88bdc7159d20bd44cdaa4b3303474839f87ec6fe7ae25c",
        archive_bytes: 38_469_316,
        executable: "deno",
        binary_sha256: "b73737579d5a84c160e3316487594783fa5c15f4e13252a6a07050b755317f1a",
        binary_bytes: 80_982_000,
    },
    Asset {
        target: "x86_64-pc-windows-msvc",
        archive: "deno-x86_64-pc-windows-msvc.zip",
        archive_sha256: "a0c3101b4158d1dfb7d6a78a7bf0f3de80c96bb423c152beec8beb22786f2238",
        archive_bytes: 42_630_221,
        executable: "deno.exe",
        binary_sha256: "e020f3e232bd16e33768dee528e5983349c962952051ced0a5d58ad42f5d9b33",
        binary_bytes: 97_462_048,
    },
    Asset {
        target: "aarch64-pc-windows-msvc",
        archive: "deno-aarch64-pc-windows-msvc.zip",
        archive_sha256: "c4c4ac8bfdaa37814bda5c05fc9cdf2154904e2ef8277673a30bdceaaa649807",
        archive_bytes: 40_847_024,
        executable: "deno.exe",
        binary_sha256: "970a5255e78f436abed194017648f8a5ab8135cbd95702ec803123c60b6a5cc0",
        binary_bytes: 88_891_168,
    },
];
