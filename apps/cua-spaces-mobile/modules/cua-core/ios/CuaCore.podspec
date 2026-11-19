Pod::Spec.new do |s|
  s.name           = 'CuaCore'
  s.version        = '0.1.0'
  s.summary        = 'The Cua Spaces app core (Rust) for React Native'
  s.description    = 'call(method, argsJson) over cua-spaces-app-core, linked as a static library.'
  s.author         = 'Cua AI, Inc.'
  s.homepage       = 'https://cua.ai'
  s.license        = { :type => 'FSL-1.1-MIT' }
  s.platforms      = { :ios => '15.1' }
  s.source         = { :git => '' }
  s.static_framework = true
  s.dependency 'ExpoModulesCore'
  s.source_files   = '*.{h,m,swift}'
  s.public_header_files = 'CuaCoreBridge.h'
  # Built by scripts/build-core-ios.sh (not committed).
  s.vendored_frameworks = 'CuaCoreLib.xcframework'
  s.pod_target_xcconfig = { 'DEFINES_MODULE' => 'YES', 'SWIFT_COMPILATION_MODE' => 'wholemodule' }
end
