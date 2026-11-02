# Velo

<div align="center">
  <img src="src-tauri/icons/icon.png" width="112" alt="Velo Logo" />

  **Move files. Not limits.**

  A modern, lightweight, and powerful download manager built with Rust, Tauri, and React.

  [![Version](https://img.shields.io/badge/version-0.7.0-00bcd4.svg)](https://github.com/ConsoleX-ir/Velo/releases)
  [![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
  ![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux-lightgrey.svg)
  ![Rust](https://img.shields.io/badge/Rust-powered-orange.svg)
  ![Tauri](https://img.shields.io/badge/Tauri-2-blue.svg)

  <br />

  [Features](#-features) · [Installation](#-installation) · [Development](#-development) · [Architecture](#-architecture) · [License](#-license)
</div>

---

## ✨ Overview

**Velo** is a modern desktop download manager designed to make downloading files fast, reliable, and straightforward.

Built with a high-performance Rust backend and a modern React interface, Velo combines efficient download management with a polished desktop experience.

The goal is simple: deliver a powerful download engine without unnecessary complexity, excessive resource consumption, advertisements, or artificial limitations.

## 🚀 Features

- **High-performance engine** — Powered by Rust for efficient download processing.
- **Download management** — Add, monitor, pause, resume, and manage downloads.
- **Multi-connection downloads** — Supports segmented HTTP downloads when server capabilities allow it.
- **Real-time progress** — Monitor download progress, speed, and status.
- **Download scheduling** — Manage when downloads should run.
- **System tray integration** — Keep Velo running in the background.
- **Desktop notifications** — Get notified about completed and failed downloads.
- **Launch at startup** — Optionally start Velo when your system boots.
- **Light & Dark themes** — Choose the appearance that suits your environment.
- **Lightweight experience** — Designed to keep resource usage under control.
- **Free and open source** — No advertisements or paid feature restrictions.

## 🛠️ Tech Stack

| Technology | Purpose |
|---|---|
| [Rust](https://www.rust-lang.org/) | Download engine and backend |
| [Tauri 2](https://tauri.app/) | Desktop application framework |
| [React](https://react.dev/) | User interface |
| [TypeScript](https://www.typescriptlang.org/) | Type-safe frontend development |
| [Vite](https://vite.dev/) | Frontend build tooling |
| [Tailwind CSS v4](https://tailwindcss.com/) | UI styling |

## 📦 Installation

Download the latest release from the [GitHub Releases](https://github.com/ConsoleX-ir/Velo/releases) page.

### Windows

Download and run the Windows installer (`.exe`).

### Linux

Choose the package that matches your distribution:

| Format | Description |
|---|---|
| `.AppImage` | Portable application package |
| `.deb` | Debian and Ubuntu-based distributions |

> **Note:** Available packages depend on the artifacts published with each release.

## 💻 Development

Want to build Velo from source or contribute to its development?

### Prerequisites

Make sure you have the following tools installed:

- [Node.js](https://nodejs.org/) — 22.12 or newer
- [npm](https://www.npmjs.com/)
- [Rust](https://www.rust-lang.org/tools/install)
- [Tauri prerequisites](https://tauri.app/start/prerequisites/)

### Clone the repository

```bash
git clone https://github.com/ConsoleX-ir/Velo.git
cd Velo
```

### Install dependencies

```bash
npm install
```

### Run in development mode

```bash
npm run tauri dev
```

### Build the application

```bash
npm run tauri build
```

### Run tests

```bash
cargo test
```

For frontend tests:

```bash
npm test
```

## 🏗️ Architecture

Velo uses a layered architecture that separates the download engine, desktop integration, and frontend interface.

```text
              ┌──────────────────────┐
              │      React UI        │
              │   TypeScript + Vite  │
              └──────────┬───────────┘
                         │
                    Tauri IPC
                         │
              ┌──────────▼───────────┐
              │    Tauri Backend     │
              │   Commands & Events  │
              └──────────┬───────────┘
                         │
              ┌──────────▼───────────┐
              │   Download Manager   │
              │   Queue & Lifecycle  │
              └──────────┬───────────┘
                         │
              ┌──────────▼───────────┐
              │   Rust Download      │
              │       Engine         │
              └──────────────────────┘
```

This separation keeps the core download logic independent from the desktop interface and makes the application easier to maintain and extend.

## 📁 Project Structure

```text
Velo/
├── src/                  # React frontend
├── src-tauri/            # Tauri application and Rust backend
├── scripts/              # Build and packaging scripts
├── installer/            # Release installer artifacts
├── package.json
└── README.md
```

## 🔒 Privacy & Security

Velo is designed with a straightforward approach to privacy:

- No advertisements.
- No unnecessary account requirements.
- No paid feature restrictions.
- Download processing is handled locally by the application.

Always download Velo from the official repository or its GitHub Releases page.

## 🗺️ Roadmap

Velo is actively evolving. Future development may include:

- Further download engine optimizations.
- Improved download recovery and reliability.
- Additional protocol support.
- More advanced download management features.
- Continued UI and performance improvements.

## 🤝 Contributing

Contributions, bug reports, and feature suggestions are welcome!

1. Fork the repository.
2. Create a new branch:

   ```bash
   git checkout -b feature/your-feature
   ```

3. Commit your changes:

   ```bash
   git commit -m "feat: add your feature"
   ```

4. Push your branch and open a Pull Request.

For bug reports and feature requests, use [GitHub Issues](https://github.com/ConsoleX-ir/Velo/issues).

## 📄 License

Velo is licensed under the **MIT License**.

See the [LICENSE](LICENSE) file for the full license text.

---

<div align="center">

  **Velo — Move files. Not limits.**

  Built with ❤️ by [ConsoleX](https://github.com/ConsoleX-ir)

  <sub>Fast. Reliable. Free.</sub>

</div>
