import { Router, type IRouter } from "express";
import healthRouter from "./health";
import networkRouter from "./network";
import blocksRouter from "./blocks";
import transactionsRouter from "./transactions";
import validatorsRouter from "./validators";
import defiRouter from "./defi";
import aiRouter from "./ai";
import xclRouter from "./xcl";
import oracleRouter from "./oracle";
import governanceRouter from "./governance";
import nftRouter from "./nft";
import payidRouter from "./payid";
import mempoolRouter from "./mempool";

const router: IRouter = Router();

router.use(healthRouter);
router.use(networkRouter);
router.use(blocksRouter);
router.use(transactionsRouter);
router.use(validatorsRouter);
router.use(defiRouter);
router.use(aiRouter);
router.use(xclRouter);
router.use(oracleRouter);
router.use(governanceRouter);
router.use(nftRouter);
router.use(payidRouter);
router.use(mempoolRouter);

export default router;
