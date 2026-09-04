import Dispatch
import NetworkExtension

// Network Extension system extensions do not use NSApplicationMain. Apple asks
// providers to enter system-extension mode as early as possible, then keep the
// process alive while nesessionmanager creates the provider classes declared in
// Info.plist.
NEProvider.startSystemExtensionMode()
dispatchMain()
