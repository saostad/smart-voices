import Testing
@testable import SmartVoicesCore

@Suite
struct AutoReadPolicyTests {
    @Test func readsWhenEnabledWithUnreadContentAndNotExporting() {
        #expect(
            AutoReadPolicy.shouldRead(
                autoReadEnabled: true, hasUnreadContent: true, isExporting: false))
    }

    @Test func doesNotReadWhenDisabled() {
        #expect(
            !AutoReadPolicy.shouldRead(
                autoReadEnabled: false, hasUnreadContent: true, isExporting: false))
    }

    @Test func doesNotReadWithoutUnreadContent() {
        #expect(
            !AutoReadPolicy.shouldRead(
                autoReadEnabled: true, hasUnreadContent: false, isExporting: false))
    }

    @Test func doesNotReadWhileExporting() {
        #expect(
            !AutoReadPolicy.shouldRead(
                autoReadEnabled: true, hasUnreadContent: true, isExporting: true))
    }
}
