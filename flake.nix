{
  description = "Iced development environment";

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
