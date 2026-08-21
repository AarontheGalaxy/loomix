# Developer commands. docs/SPEC.md is the source of truth for what these
# wrap; this file is just a shorthand over the same commands CI runs.

build:
    cargo build --workspace --all-features
    xcodebuild -project driver/LoomixAudioDriver.xcodeproj -scheme LoomixAudioDriver -configuration Debug -derivedDataPath driver/build build CODE_SIGNING_ALLOWED=NO

test:
    cargo test --workspace --all-features
    cargo test --workspace --release -- --ignored golden
    mkdir -p driver/build
    clang -Wall -Wextra -Werror -std=gnu17 -o driver/build/test_ring_buffer driver/tests/test_ring_buffer.c driver/LoomixAudioDriver/RingBuffer.c
    driver/build/test_ring_buffer
    clang -Wall -Wextra -Werror -std=gnu17 -framework CoreFoundation -framework CoreAudio -o driver/build/test_driver_host driver/tests/test_driver_host.c driver/LoomixAudioDriver/LoomixAudioDriver.c driver/LoomixAudioDriver/RingBuffer.c
    driver/build/test_driver_host
    clang -Wall -Wextra -Werror -std=gnu17 -DLOOMIX_CALLOC=FailingCalloc -DLOOMIX_MALLOC=FailingMalloc -framework CoreFoundation -framework CoreAudio -o driver/build/test_driver_host_fault_injection driver/tests/test_driver_host.c driver/LoomixAudioDriver/LoomixAudioDriver.c driver/LoomixAudioDriver/RingBuffer.c
    driver/build/test_driver_host_fault_injection

lint:
    cargo fmt --all --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo deny check
    ./driver/tests/run-static-checks.sh
    npm run --prefix ui typecheck
    npm run --prefix ui lint

cover:
    cargo llvm-cov --workspace --lcov --output-path lcov.info --fail-under-lines 80

bench:
    cargo bench -p loomix-core -- --save-baseline pr
    ./scripts/check-bench-regression.sh 10

# Builds an ad-hoc-signed Release driver and installs it to
# /Library/Audio/Plug-Ins/HAL/. Needs sudo; restarts coreaudiod, which
# briefly interrupts all audio on the machine (spec section 2.1).
install-driver:
    xcodebuild -project driver/LoomixAudioDriver.xcodeproj -scheme LoomixAudioDriver -configuration Release -derivedDataPath driver/build build CODE_SIGN_IDENTITY=-
    ./driver/scripts/install.sh

# Removes the installed driver. Needs sudo; restarts coreaudiod.
uninstall-driver:
    ./driver/scripts/uninstall.sh

# Restarts the CoreAudio daemon. Needs sudo; briefly interrupts all audio
# on the machine.
restart-coreaudio:
    sudo killall coreaudiod
