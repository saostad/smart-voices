/// A new copy replaces whatever is being read.
public enum AutoReadPolicy {
    public static func shouldRead(
        autoReadEnabled: Bool, hasUnreadContent: Bool, isExporting: Bool
    ) -> Bool {
        autoReadEnabled && hasUnreadContent && !isExporting
    }
}
