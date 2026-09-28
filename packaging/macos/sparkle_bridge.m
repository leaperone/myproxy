#import <Foundation/Foundation.h>
#import <Sparkle/Sparkle.h>

static SPUStandardUpdaterController *gController;
static NSString *gFeedURL;
static NSInteger gChannel;
extern void myproxy_sparkle_mark_update_resume(void);
extern unsigned long long myproxy_sparkle_begin_check(void);
extern void myproxy_sparkle_event(unsigned long long operation, int event, const char *value);

@interface MyproxyUpdaterDelegate : NSObject <SPUUpdaterDelegate>
@property(nonatomic) unsigned long long operation;
@end

@implementation MyproxyUpdaterDelegate
- (BOOL)updater:(SPUUpdater *)updater mayPerformUpdateCheck:(SPUUpdateCheck)updateCheck error:(NSError * __autoreleasing *)error {
    (void)updater;
    (void)updateCheck;
    (void)error;
    self.operation = myproxy_sparkle_begin_check();
    return YES;
}
- (void)updater:(SPUUpdater *)updater didFindValidUpdate:(SUAppcastItem *)item {
    (void)updater;
    myproxy_sparkle_event(self.operation, 1, item.displayVersionString.UTF8String);
}
- (void)updaterDidNotFindUpdate:(SPUUpdater *)updater error:(NSError *)error {
    (void)updater;
    (void)error;
    myproxy_sparkle_event(self.operation, 2, NULL);
}
- (void)updater:(SPUUpdater *)updater didDownloadUpdate:(SUAppcastItem *)item {
    (void)updater;
    (void)item;
    myproxy_sparkle_event(self.operation, 3, NULL);
}
- (void)updater:(SPUUpdater *)updater willInstallUpdate:(SUAppcastItem *)item {
    (void)updater;
    (void)item;
    myproxy_sparkle_event(self.operation, 4, NULL);
}
- (void)updater:(SPUUpdater *)updater didAbortWithError:(NSError *)error {
    (void)updater;
    if (error.code == SUNoUpdateError) {
        myproxy_sparkle_event(self.operation, 2, NULL);
        return;
    }
    if (error.code == SUInstallationCanceledError) {
        myproxy_sparkle_event(self.operation, 6, NULL);
        return;
    }
    NSString *reason = [NSString stringWithFormat:@"%@ %ld", error.domain, (long)error.code];
    myproxy_sparkle_event(self.operation, 5, reason.UTF8String);
}
- (void)userDidCancelDownload:(SPUUpdater *)updater {
    (void)updater;
    myproxy_sparkle_event(self.operation, 6, NULL);
}
- (NSString *)feedURLStringForUpdater:(SPUUpdater *)updater {
    (void)updater;
    return [NSString stringWithFormat:@"%@?operation=%llu", gFeedURL, self.operation];
}
- (NSSet<NSString *> *)allowedChannelsForUpdater:(SPUUpdater *)updater {
    (void)updater;
    if (gChannel == 1) return [NSSet setWithObject:@"nightly"];
    if (gChannel == 2) return [NSSet setWithObject:@"xray"];
    return [NSSet set];
}
- (void)updaterWillRelaunchApplication:(SPUUpdater *)updater {
    (void)updater;
    myproxy_sparkle_mark_update_resume();
}
@end

static MyproxyUpdaterDelegate *gDelegate;

void myproxy_sparkle_set_channel(const char *feedURL, int channel) {
    @autoreleasepool {
        NSString *next = [NSString stringWithUTF8String:feedURL];
        if ([gFeedURL isEqualToString:next] && gChannel == channel) {
            return;
        }
        gFeedURL = next;
        gChannel = channel;
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
        if (gController.updater.canCheckForUpdates) {
            [gController checkForUpdates:nil];
        }
    }
}
