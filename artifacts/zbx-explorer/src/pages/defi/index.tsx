import React from "react";
import { useGetDefiStats, useGetAmmPools, useGetLendingMarkets, useGetPerpMarkets } from "@workspace/api-client-react";
import { formatNumber, formatCurrency } from "@/lib/utils";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { PieChart, DollarSign, Activity, TrendingUp, BarChart2 } from "lucide-react";
import { Progress } from "@/components/ui/progress";

export default function Defi() {
  const { data: stats, isLoading: statsLoading } = useGetDefiStats();
  const { data: amms, isLoading: ammsLoading } = useGetAmmPools();
  const { data: lending, isLoading: lendingLoading } = useGetLendingMarkets();
  const { data: perps, isLoading: perpsLoading } = useGetPerpMarkets();

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-2xl font-bold tracking-tight">DeFi Hub</h2>
        <p className="text-muted-foreground">Native decentralized finance ecosystem on Zebvix.</p>
      </div>

      <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><DollarSign className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Total TVL</p>
              <h3 className="text-xl font-bold font-mono">
                {statsLoading ? <Skeleton className="h-7 w-24" /> : formatCurrency(stats?.totalTvl || 0)}
              </h3>
            </div>
          </CardContent>
        </Card>
        
        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><Activity className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">24h Volume</p>
              <h3 className="text-xl font-bold font-mono">
                {statsLoading ? <Skeleton className="h-7 w-24" /> : formatCurrency(stats?.totalVolume24h || 0)}
              </h3>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><PieChart className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Protocols</p>
              <h3 className="text-xl font-bold font-mono">
                {statsLoading ? <Skeleton className="h-7 w-12" /> : stats?.totalProtocols || 0}
              </h3>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><TrendingUp className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Perp Vol (24h)</p>
              <h3 className="text-xl font-bold font-mono">
                {statsLoading ? <Skeleton className="h-7 w-24" /> : formatCurrency(stats?.perpVolume24h || 0)}
              </h3>
            </div>
          </CardContent>
        </Card>
      </div>

      <Tabs defaultValue="amm" className="w-full">
        <TabsList className="grid w-full md:w-auto grid-cols-3">
          <TabsTrigger value="amm">AMM Pools</TabsTrigger>
          <TabsTrigger value="lending">Lending Markets</TabsTrigger>
          <TabsTrigger value="perp">Perpetual Futures</TabsTrigger>
        </TabsList>
        
        <TabsContent value="amm" className="mt-4">
          <Card>
            <CardContent className="p-0">
              {ammsLoading ? (
                <div className="p-4 space-y-2">
                  {Array.from({ length: 5 }).map((_, i) => <Skeleton key={i} className="h-12 w-full" />)}
                </div>
              ) : (
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Pool Pair</TableHead>
                      <TableHead className="text-right">TVL</TableHead>
                      <TableHead className="text-right">24h Volume</TableHead>
                      <TableHead className="text-right">Fee</TableHead>
                      <TableHead className="text-right">APY</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {amms?.map((pool) => (
                      <TableRow key={pool.id}>
                        <TableCell className="font-bold">
                          {pool.token0} / {pool.token1}
                        </TableCell>
                        <TableCell className="text-right font-mono">{formatCurrency(pool.tvl)}</TableCell>
                        <TableCell className="text-right font-mono">{formatCurrency(pool.volume24h)}</TableCell>
                        <TableCell className="text-right font-mono">{pool.fee}%</TableCell>
                        <TableCell className="text-right font-mono text-green-500">{pool.apy}%</TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              )}
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="lending" className="mt-4">
          <Card>
            <CardContent className="p-0">
              {lendingLoading ? (
                <div className="p-4 space-y-2">
                  {Array.from({ length: 5 }).map((_, i) => <Skeleton key={i} className="h-12 w-full" />)}
                </div>
              ) : (
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Asset</TableHead>
                      <TableHead className="text-right">Total Supply</TableHead>
                      <TableHead className="text-right">Total Borrow</TableHead>
                      <TableHead>Utilization</TableHead>
                      <TableHead className="text-right">Supply APY</TableHead>
                      <TableHead className="text-right">Borrow APY</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {lending?.map((market) => (
                      <TableRow key={market.asset}>
                        <TableCell className="font-bold">{market.asset}</TableCell>
                        <TableCell className="text-right font-mono">{formatCurrency(market.totalSupply)}</TableCell>
                        <TableCell className="text-right font-mono">{formatCurrency(market.totalBorrow)}</TableCell>
                        <TableCell className="w-[150px]">
                          <div className="flex items-center gap-2">
                            <Progress value={market.utilization * 100} className="h-2" />
                            <span className="text-xs font-mono w-10">{(market.utilization * 100).toFixed(0)}%</span>
                          </div>
                        </TableCell>
                        <TableCell className="text-right font-mono text-green-500">{market.supplyApy}%</TableCell>
                        <TableCell className="text-right font-mono text-destructive">{market.borrowApy}%</TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              )}
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="perp" className="mt-4">
          <Card>
            <CardContent className="p-0">
              {perpsLoading ? (
                <div className="p-4 space-y-2">
                  {Array.from({ length: 5 }).map((_, i) => <Skeleton key={i} className="h-12 w-full" />)}
                </div>
              ) : (
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Market</TableHead>
                      <TableHead className="text-right">Price</TableHead>
                      <TableHead className="text-right">24h Change</TableHead>
                      <TableHead className="text-right">24h Volume</TableHead>
                      <TableHead className="text-right">Open Interest</TableHead>
                      <TableHead className="text-right">Funding (1h)</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {perps?.map((market) => (
                      <TableRow key={market.symbol}>
                        <TableCell className="font-bold">{market.symbol}</TableCell>
                        <TableCell className="text-right font-mono">{formatCurrency(market.price, 4)}</TableCell>
                        <TableCell className={`text-right font-mono ${market.change24h >= 0 ? 'text-green-500' : 'text-destructive'}`}>
                          {market.change24h > 0 ? '+' : ''}{market.change24h}%
                        </TableCell>
                        <TableCell className="text-right font-mono">{formatCurrency(market.volume24h)}</TableCell>
                        <TableCell className="text-right font-mono">{formatCurrency(market.openInterest)}</TableCell>
                        <TableCell className={`text-right font-mono ${market.fundingRate >= 0 ? 'text-green-500' : 'text-destructive'}`}>
                          {market.fundingRate}%
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              )}
            </CardContent>
          </Card>
        </TabsContent>
      </Tabs>
    </div>
  );
}
