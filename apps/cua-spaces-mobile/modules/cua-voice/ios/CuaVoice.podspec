Pod::Spec.new do |s|
  s.name           = 'CuaVoice'
  s.version        = '0.1.0'
  s.summary        = 'Push-to-talk dictation for Cua Spaces (the phone speech recognizer)'
  s.description    = 'SFSpeechRecognizer over AVAudioEngine, on the phone where the language supports it. No audio is sent to Cua.'
  s.author         = 'Cua AI, Inc.'
  s.homepage       = 'https://cua.ai'
  s.license        = { :type => 'FSL-1.1-MIT' }
  s.platforms      = { :ios => '15.1' }
  s.source         = { :git => '' }
  s.static_framework = true
  s.dependency 'ExpoModulesCore'
  s.source_files   = '*.{h,m,swift}'
  s.frameworks     = 'Speech', 'AVFoundation'
  s.pod_target_xcconfig = { 'DEFINES_MODULE' => 'YES', 'SWIFT_COMPILATION_MODE' => 'wholemodule' }
end
