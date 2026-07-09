{
  description = "Bridges LangChain OpenWiki into the pleme-io fleet: reads the target repo's typescape IR + zoekt hits as context, invokes OpenWiki unmodified as a subprocess, routes the result through the fleet's doc/compliance layers.";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    crate2nix.url = "github:nix-community/crate2nix";
    flake-utils.url = "github:numtide/flake-utils";
    substrate = {
      url = "github:pleme-io/substrate";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.fenix.follows = "fenix";
    };
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, crate2nix, flake-utils, substrate, ... }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" "x86_64-linux" "aarch64-linux" ];

      rustOutputs = (import "${substrate}/lib/rust-tool-release-flake.nix" {
        inherit nixpkgs crate2nix flake-utils;
      }) {
        toolName = "ponte";
        src = self;
        repo = "pleme-io/ponte";
        inherit systems;
      };

      # OpenWiki (langchain-ai/openwiki) is unmodified, external, upstream
      # source — never forked. It's a pnpm project (pnpm-lock.yaml +
      # pnpm-workspace.yaml), not npm, so it's packaged via substrate's
      # mkPnpmTool (wraps nixpkgs' native pnpm.fetchDeps + configHook),
      # not mkNpmTool (buildNpmPackage only understands package-lock.json).
      openwikiRev = "d417b5c759240cec63f1c5b29b3f91777958858a";
      openwikiSrcHash = "sha256-Aj+dwj8ljx4l9aC9OalqUMzMbnISzTQhUPLB2KHpC3M=";
      openwikiPnpmDepsHash = "sha256-/p3a9gzpPWtFM/F3Udh3n0k97CrSX+UVHubZN/ZUZgU=";

      openwikiFor = system: let
        pkgs = import nixpkgs { inherit system; };
        pnpmToolBuilder = import "${substrate}/lib/build/npm/pnpm-tool.nix";
        src = pkgs.fetchFromGitHub {
          owner = "langchain-ai";
          repo = "openwiki";
          rev = openwikiRev;
          hash = openwikiSrcHash;
        };
      in pnpmToolBuilder.mkPnpmTool pkgs {
        pname = "openwiki";
        version = "0.0.3";
        inherit src;
        pnpmDepsHash = openwikiPnpmDepsHash;
        binEntry = "dist/cli.js";
        homepage = "https://github.com/langchain-ai/openwiki";
        description = "OpenWiki — a DeepAgents CLI that writes and maintains agent documentation for a codebase (unmodified upstream, packaged for ponte)";
      };

      openwikiPackages = nixpkgs.lib.genAttrs systems (system: {
        openwiki = openwikiFor system;
      });
    in
      nixpkgs.lib.recursiveUpdate rustOutputs {
        packages = openwikiPackages;
      };
}
