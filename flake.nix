{
  description = "Rust devshell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      rust-overlay,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        rust = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "rust-src"
            "rustfmt"
            "clippy"
          ];
          targets = [ "wasm32-unknown-unknown" ];
        };

        whisper-cpp-local = pkgs.stdenv.mkDerivation rec {
          pname = "whisper-cpp";
          version = "1.8.3";

          src = pkgs.fetchFromGitHub {
            owner = "ggml-org";
            repo = "whisper.cpp";
            rev = "v${version}";
            hash = "sha256-TeS1lGKEzkHOoBemy/tMGtIsy0iouj9DTYIgTjUNcQk=";
          };

          nativeBuildInputs = [ pkgs.cmake ];

          buildInputs = [ ];

          cmakeFlags = [
            "-DWHISPER_BUILD_EXAMPLES=ON"
            "-DWHISPER_BUILD_SERVER=ON"
            "-DGGML_METAL=ON"
            "-DGGML_COREML=OFF"
            "-DGGML_NATIVE=OFF"
          ];

          meta.mainProgram = "whisper-server";
        };

      in
      {
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            rust
            wasm-bindgen-cli
            rust-analyzer # LSP for editors
            cargo-edit # cargo add/rm/upgrade
            cargo-watch # auto rebuild/test

            llama-cpp # for testing worker backends
            whisper-cpp-local # for testing transcription backend
            ffmpeg # for audio processing in transcription backend
          ];

          # Useful env vars for crates using openssl-sys / pkg-config:
          shellHook = ''
            export RUST_BACKTRACE=1
            echo "Rust: $(rustc --version)"
            echo "Cargo: $(cargo --version)"
          '';
        };
      }
    );
}
