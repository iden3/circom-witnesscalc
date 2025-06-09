all: libs-all copy-libs

libs-all: lib-ios lib-ios-sim lib-android lib-macos

lib-ios: lib-ios-device lib-ios-sim

lib-ios-device:
	cargo build --target aarch64-apple-ios --release

lib-ios-sim:
	cargo build --target x86_64-apple-ios --release
	cargo build --target aarch64-apple-ios-sim --release

lib-android: lib-android-aarch64 lib-android-x86_64

lib-android-aarch64:
	export CC=${ANDROID_NDK}/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android34-clang; \
	export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$$CC; \
	export CLANG_PATH=$$CC; \
	cargo build --target aarch64-linux-android --release

lib-android-x86_64:
	export CC=${ANDROID_NDK}/toolchains/llvm/prebuilt/darwin-x86_64/bin/x86_64-linux-android34-clang; \
	export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER=$$CC; \
	export CLANG_PATH=$$CC; \
	cargo build --target x86_64-linux-android --release

lib-macos:
	cargo build --target aarch64-apple-darwin --release --workspace

copy-libs:
	mkdir -p \
		circom-witnesscalc/aarch64-apple-darwin \
		circom-witnesscalc/aarch64-apple-ios \
		circom-witnesscalc/aarch64-apple-ios-sim \
		circom-witnesscalc/aarch64-linux-android \
		circom-witnesscalc/x86_64-apple-ios \
		circom-witnesscalc/x86_64-linux-android
	# macOS
	cp \
		target/aarch64-apple-darwin/release/build-circuit \
		target/aarch64-apple-darwin/release/calc-witness \
		target/aarch64-apple-darwin/release/libcircom_witnesscalc.a \
		target/aarch64-apple-darwin/release/libcircom_witnesscalc.dylib \
		circom-witnesscalc/aarch64-apple-darwin
	install_name_tool -id @rpath/libcircom_witnesscalc.dylib circom-witnesscalc/aarch64-apple-darwin/libcircom_witnesscalc.dylib
	# iOS
	cp \
		target/aarch64-apple-ios/release/libcircom_witnesscalc.a \
		target/aarch64-apple-ios/release/libcircom_witnesscalc.dylib \
		circom-witnesscalc/aarch64-apple-ios
	install_name_tool -id @rpath/libcircom_witnesscalc.dylib circom-witnesscalc/aarch64-apple-ios/libcircom_witnesscalc.dylib
	# iOS Simulator ARM
	cp \
		target/aarch64-apple-ios-sim/release/libcircom_witnesscalc.a \
		target/aarch64-apple-ios-sim/release/libcircom_witnesscalc.dylib \
		circom-witnesscalc/aarch64-apple-ios-sim
	install_name_tool -id @rpath/libcircom_witnesscalc.dylib circom-witnesscalc/aarch64-apple-ios-sim/libcircom_witnesscalc.dylib
	# iOS Simulator x86_64
	cp \
		target/x86_64-apple-ios/release/libcircom_witnesscalc.a \
		target/x86_64-apple-ios/release/libcircom_witnesscalc.dylib \
		circom-witnesscalc/x86_64-apple-ios
	install_name_tool -id @rpath/libcircom_witnesscalc.dylib circom-witnesscalc/x86_64-apple-ios/libcircom_witnesscalc.dylib
	# Android ARM
	cp \
		target/aarch64-linux-android/release/libcircom_witnesscalc.a \
		target/aarch64-linux-android/release/libcircom_witnesscalc.so \
		circom-witnesscalc/aarch64-linux-android
	# We have an error load library on android after patchelf. The -soname parameter was added to build.rs and this
	# was commented for now.
	# patchelf --set-soname libcircom_witnesscalc.so circom-witnesscalc/aarch64-linux-android/libcircom_witnesscalc.so
	# Android x86_64
	cp \
		target/x86_64-linux-android/release/libcircom_witnesscalc.a \
		target/x86_64-linux-android/release/libcircom_witnesscalc.so \
		circom-witnesscalc/x86_64-linux-android
	# We have an error load library on android after patchelf. The -soname parameter was added to build.rs and this
	# was commented for now.
	# patchelf --set-soname libcircom_witnesscalc.so circom-witnesscalc/x86_64-linux-android/libcircom_witnesscalc.so
