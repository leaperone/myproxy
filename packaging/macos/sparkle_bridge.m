#import <Foundation/Foundation.h>
#import <Sparkle/Sparkle.h>
#import <objc/runtime.h>

static SPUStandardUpdaterController *gController;
static NSString *gFeedURL;
static BOOL gNightly;
static NSString *gProxyHost;
static NSInteger gProxyPort;
static NSLock *gProxyLock;

static NSLock *MyproxyProxyLock(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        gProxyLock = [[NSLock alloc] init];
    });
    return gProxyLock;
}

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
    NSString *host;
    NSInteger port;
    [MyproxyProxyLock() lock];
    host = gProxyHost;
    port = gProxyPort;
    [MyproxyProxyLock() unlock];
    if (host.length == 0 || port <= 0) {
        return;
    }
    config.connectionProxyDictionary = @{
        @"HTTPEnable": @YES,
        @"HTTPProxy": host,
        @"HTTPPort": @(port),
        @"HTTPSEnable": @YES,
        @"HTTPSProxy": host,
        @"HTTPSPort": @(port),
        @"ExceptionsList": @[@"127.0.0.1", @"localhost", @"*.local"],
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
        NSString *next = nil;
        NSInteger nextPort = 0;
        if (host != NULL && port > 0 && port <= 65535) {
            next = [NSString stringWithUTF8String:host];
            nextPort = port;
        }
        [MyproxyProxyLock() lock];
        if ((gProxyHost == next || [gProxyHost isEqualToString:next]) && gProxyPort == nextPort) {
            [MyproxyProxyLock() unlock];
            return;
        }
        gProxyHost = next;
        gProxyPort = nextPort;
        [MyproxyProxyLock() unlock];
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
