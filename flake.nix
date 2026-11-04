{
  description = "EVE Alert 软件包与开发环境";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
    in
    {
      packages.${system} = rec {
        default = eve-alert;
        eve-alert = pkgs.rustPlatform.buildRustPackage (finalAttrs: {
          pname = "eve-alert";
          version = (builtins.fromJSON (builtins.readFile ./src-tauri/tauri.conf.json)).version;

          # 仅包含构建输入，避免收录本地配置、捕获凭据与生成目录。
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./src
              ./public
              ./index.html
              ./package.json
              ./pnpm-lock.yaml
              ./tsconfig.json
              ./tsconfig.app.json
              ./tsconfig.node.json
              ./vite.config.ts
              ./src-tauri/src
              ./src-tauri/build.rs
              ./src-tauri/Cargo.toml
              ./src-tauri/Cargo.lock
              ./src-tauri/tauri.conf.json
              ./src-tauri/capabilities
              ./src-tauri/icons
            ];
          };

          cargoRoot = "src-tauri";
          buildAndTestSubdir = finalAttrs.cargoRoot;
          cargoLock.lockFile = ./src-tauri/Cargo.lock;

          pnpmDeps = pkgs.fetchPnpmDeps {
            inherit (finalAttrs) pname version src;
            pnpm = pkgs.pnpm_11;
            fetcherVersion = 5;
            hash = "sha256-YM99vVvchY9tbfoVFoYshwn/31PdjVIo8gzkLyl3lbI=";
          };

          nativeBuildInputs = with pkgs; [
            cargo-tauri.hook
            nodejs_26
            pnpm_11
            pnpmConfigHook
            pkg-config
            clang
            rustPlatform.bindgenHook
            wrapGAppsHook3
          ];

          buildInputs = with pkgs; [
            pipewire
            gtk3
            webkitgtk_4_1
            glib-networking
            gst_all_1.gstreamer
            gst_all_1.gst-plugins-base
            gst_all_1.gst-plugins-good
            gst_all_1.gst-plugins-bad
          ];

          # HTTP 客户端初始化时需要读取 CA 证书；构建沙箱不提供系统证书。
          nativeCheckInputs = [ pkgs.cacert ];

          # Cargo crate 名称为 app；安装后的命令统一使用产品名。
          tauriBuildFlags = [
            "--config"
            (builtins.toJSON { mainBinaryName = "eve-alert"; })
          ];

          postInstall = ''
            substituteInPlace "$out/share/applications/eve-alert.desktop" \
              --replace-fail "Name=eve-alert" "Name=EVE Alert"
          '';

          preFixup = ''
            gappsWrapperArgs+=(
              --set-default WEBKIT_DISABLE_DMABUF_RENDERER 1
              --set-default GDK_BACKEND x11
            )
          '';

          meta = {
            description = "面向 EVE Online 的屏幕 RGB 识别与告警桌面应用";
            homepage = "https://github.com/Ecamika/eve-alert";
            mainProgram = "eve-alert";
            platforms = [ system ];
          };
        });
      };

      devShells.${system}.default = pkgs.mkShell {
        nativeBuildInputs = with pkgs; [
          pkg-config
          clang
          # 为 bindgen 配置 libclang 路径及 Clang/系统头文件参数。
          rustPlatform.bindgenHook
        ];

        buildInputs = with pkgs; [
          # 提供 PipeWire/SPA 头文件、库及 pkg-config 元数据。
          pipewire

          wayland
          libxkbcommon

          vulkan-loader

          libX11
          libXcursor
          libXi
          libXrandr

          glibc
          glib

          cairo
          pango
          atk
          gdk-pixbuf
          gdk-pixbuf-xlib
          gtk3
          libsoup_3
          webkitgtk_4_1

          gst_all_1.gstreamer
          gst_all_1.gst-plugins-base
          gst_all_1.gst-plugins-bad
          gst_all_1.gst-plugins-good
        ];

        packages = with pkgs; [
          # Rust Toolchain
          rustc
          cargo
          rustfmt
          clippy
          cargo-tauri
          rust-analyzer

          # Nodejs Toolchain
          vscode-css-languageserver
          typescript-language-server
          nodejs_26
          pnpm_11
          flow
          prettier
          prettierd

          oxfmt
          oxlint

        ];

        LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (
          with pkgs;
          [
            wayland
            libxkbcommon
            vulkan-loader

            libX11
            libXcursor
            libXi
            libXrandr
          ]
        );

        shellHook = ''
          export WEBKIT_DISABLE_DMABUF_RENDERER=1
          export GDK_BACKEND=x11
        '';
      };
    };
}
