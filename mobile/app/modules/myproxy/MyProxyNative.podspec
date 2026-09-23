Pod::Spec.new do |s|
  s.name           = 'MyProxyNative'
  s.version        = '0.0.10'
  s.summary        = 'MyProxy Xray native control bridge'
  s.homepage       = 'https://github.com/leaperone/myproxy'
  s.author         = 'Leaperone'
  s.license        = { :type => 'UNLICENSED' }
  s.platforms      = { :ios => '16.4' }
  s.source         = { :git => 'https://github.com/leaperone/myproxy.git', :branch => 'feat/xray-mobile' }
  s.source_files   = 'ios/*.swift'
  s.vendored_frameworks = 'ios/Frameworks/MyProxyCore.xcframework'
  s.dependency 'ExpoModulesCore'
  s.frameworks     = 'NetworkExtension', 'Security'
  s.libraries      = 'resolv'
end
