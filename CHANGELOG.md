# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/)
and this project adheres to [Semantic Versioning](http://semver.org/).

<!-- next-header -->

## [Unreleased] - ReleaseDate
- Fix Consul lock session "leak" when the inner loop performed an early return [#212](https://github.com/svenstaro/wiresmith/pull/212)

## [0.4.2] - 2024-07-05
- Delete `--consul-ttl` argument. The TTL decision affects core logic so it doesn't make much sense to adjust it.
- Make Consul logic more reliable

## [0.4.1] - 2024-07-03
- More gracefully handle Consul errors which are more likely to occurr since 0.4.0 [#209](https://github.com/svenstaro/wiresmith/pull/209)

## [0.4.0] - 2024-07-03
- Remove self from the Consul state when wiresmith shuts down [#200](https://github.com/svenstaro/wiresmith/pull/200) (thanks @kyrias)
- Breaking: Rework Consul integration to use sessions and work across datacenters. This is a breaking change for setups that connect a mesh across Consul datacenters. [#207](https://github.com/svenstaro/wiresmith/pull/207)

## [0.3.0] - 2024-04-12
- Use latest data transmission date instead of latest handshake for timeouts [#17](https://github.com/svenstaro/wiresmith/pull/17) (thanks @tomgroenwoldt)
- Added `--consul-datacenter` option

## [0.2.1] - 2023-04-17
- Fix peer deletion

## [0.2.0] - 2023-04-13
- Validate address is inside network
- Add locking mechanism when interacting with Consul
- Improve logging

## [0.1.2] - 2023-04-03
- Fix broken timeout mechanism
- Fix binary and image releases

## [0.1.0] - 2023-04-03
- Initial release

<!-- next-url -->
[Unreleased]: https://github.com/svenstaro/wiresmith/compare/v0.4.2...HEAD
[0.4.2]: https://github.com/svenstaro/wiresmith/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/svenstaro/wiresmith/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/svenstaro/wiresmith/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/svenstaro/wiresmith/compare/v0.2.1...v0.3.0
[0.2.1]: https://github.com/svenstaro/wiresmith/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/svenstaro/wiresmith/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/svenstaro/wiresmith/compare/v0.1.0...v0.1.2
