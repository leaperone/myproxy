Pod::Spec.new do |s|
  s.name           = 'MyProxy'
  s.version        = '0.1.0'
  s.summary        = 'MyProxy Xray native control bridge'
  s.platforms      = { :ios => '15.0' }
  s.source         = { :path => '.' }
  s.source_files   = 'ios/*.swift'
  s.dependency 'ExpoModulesCore'
  s.frameworks     = 'NetworkExtension'
end
