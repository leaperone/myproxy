#import <Foundation/Foundation.h>
#import <Sparkle/Sparkle.h>

static SPUStandardUpdaterController *gController;
static NSString *gFeedURL;
static BOOL gNightly;

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

void myproxy_sparkle_init(void) {
    @autoreleasepool {
        NSString *path = [[NSBundle mainBundle] bundlePath];
        if (![path hasSuffix:@".app"]) {
            return;
        }
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
