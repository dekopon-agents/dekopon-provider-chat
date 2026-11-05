# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1] - 2026-11-05

### Fixed

- `chat slack get` no longer includes the bot's own `› model · steps · time` footer in the text of its image replies. When a fetched page holds a marked footer, the read sends one query-free `GET /api/auth.test` to learn the bot's user ID, so a Slack read makes at most three requests; the broker grant needs that template and `maxRequests: 3`.

## [0.1.0] - 2026-10-16

### Added

- First release: `chat slack get last N` and `chat discord get last N` read broker-scoped text history through separately granted `chat.slack.history.read` and `chat.discord.history.read` capabilities.
