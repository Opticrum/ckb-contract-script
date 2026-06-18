use ckb_cinnabar_verifier::{define_errors, CUSTOM_ERROR_START};

define_errors!(OpticrumError, {
    // Order errors
    BadOrderCancel = CUSTOM_ERROR_START,
    BadOrderMatch,
    ChannelCellNotInDep,
    ChannelCapacityMismatch,
    OrderDataNotSet,
    BadXudtAmount,

    // Match errors
    BadExtractionAmount,
    MatchDataNotSet,
    HeaderNotSet,
    BadMatchDataUpdate,
    MatchAlreadyExpired,
    MatchAlreadyExhausted,
    MatchNotExhausted,

    // General errors
    BadArgsLength,
    BuyerAuthMissing,
    SellerAuthMissing,
    AuthorizationMissing,
    UnexpectedBranch,
    UnknownState,
});
