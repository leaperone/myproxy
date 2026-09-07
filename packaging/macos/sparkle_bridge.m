#import <Foundation/Foundation.h>
#import <Sparkle/Sparkle.h>
#import <objc/runtime.h>

static SPUStandardUpdaterController *gController;
static NSString *gFeedURL;
static BOOL gNightly;
static NSString *gProxyHost;
static NSInteger gProxyPort;

@interface MyproxyUpdaterDelegate : NSObject <SPUUpdaterDelegate>
@end

@implementation MyproxyUpdaterDelegate
- (NSString *)feedURLStringForUpdater:(SPUUpdater *)updater {
    return gFeedURL;
}
- (NSSet<NSString *> *)allowedChannelsForUpdater:(SPUUpdater *)updater {
    return gNightly ? [NSSet setWithObject:@"nightly"] : [NSSet set];
}
@end

static MyproxyUpdaterDelegate *gDelegate;
static NSURLSessionConfiguration *(*MyproxyOrigDefaultSession)(id, SEL);

static void MyproxyApplyProxy(NSURLSessionConfiguration *config) {
    if (gProxyHost.length == 0 || gProxyPort <= 0) {
        return;
    }
    config.connectionProxyDictionary = @{
        @"HTTPEnable": @YES,
        @"HTTPProxy": gProxyHost,
        @"HTTPPort": @(gProxyPort),
        @"HTTPSEnable": @YES,
        @"HTTPSProxy": gProxyHost,
        @"HTTPSPort": @(gProxyPort),
    };
}

static NSURLSessionConfiguration *MyproxyDefaultSession(id self, SEL cmd) {
    NSURLSessionConfiguration *config = MyproxyOrigDefaultSession(self, cmd);
    MyproxyApplyProxy(config);
    return config;
}

static void MyproxyInstallSessionProxy(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        Method method = class_getClassMethod(
            [NSURLSessionConfiguration class],
            @selector(defaultSessionConfiguration)
        );
        if (method == NULL) {
            return;
        }
        MyproxyOrigDefaultSession = (void *)method_getImplementation(method);
        method_setImplementation(method, (IMP)MyproxyDefaultSession);
    });
}

void myproxy_sparkle_set_channel(const char *feedURL, int nightly) {
    @autoreleasepool {
        NSString *next = [NSString stringWithUTF8String:feedURL];
        if ([gFeedURL isEqualToString:next] && gNightly == (nightly != 0)) {
            return;
        }
        gFeedURL = next;
        gNightly = nightly != 0;
        [gController.updater resetUpdateCycleAfterShortDelay];
    }
}

void myproxy_sparkle_set_proxy(const char *host, int port) {
    @autoreleasepool {
        MyproxyInstallSessionProxy();
        if (host == NULL || port <= 0 || port > 65535) {
            gProxyHost = nil;
            gProxyPort = 0;
            return;
        }
        NSString *next = [NSString stringWithUTF8String:host];
        if ([gProxyHost isEqualToString:next] && gProxyPort == port) {
            return;
        }
        gProxyHost = next;
        gProxyPort = port;
    }
}

void myproxy_sparkle_init(void) {
    @autoreleasepool {
        NSString *path = [[NSBundle mainBundle] bundlePath];
        if (![path hasSuffix:@".app"]) {
            return;
        }
        MyproxyInstallSessionProxy();
        if (gController != nil) {
            return;
        }
        gDelegate = [[MyproxyUpdaterDelegate alloc] init];
        gController = [[SPUStandardUpdaterController alloc]
            initWithStartingUpdater:YES
                    updaterDelegate:gDelegate
                 userDriverDelegate:nil];
    }
}

void myproxy_sparkle_check(void) {
    @autoreleasepool {
        myproxy_sparkle_init();
        if (gController == nil) {
            return;
        }
        [gController checkForUpdates:nil];
    }
}
